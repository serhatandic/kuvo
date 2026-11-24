//! Where a conversation's messages go: the engine's inbox, or the line the
//! coding agents keep for theirs. The queue asks both the same questions.

use std::{io, sync::Arc, time::Duration};

use http::{Method, StatusCode};
use serde_json::{Map, Value};

use super::{Delivery, Inbox, Prompt};
use crate::{
    AgentLinePrompt, AgentSessionSource, AgentTakeBack, Engine, api,
    lifetime::{Demand, Lease},
    sessions,
};

/// The owned place a command works on.
pub(crate) enum Place {
    Engine(Lease),
    Agents(Arc<dyn AgentSessionSource>),
}

impl Place {
    pub(crate) fn target(&self) -> Target<'_> {
        match self {
            Self::Engine(lease) => Target::Engine(lease),
            Self::Agents(source) => Target::Agents(source.as_ref()),
        }
    }

    /// The coding agents' place for their conversation, if it's theirs.
    pub(crate) async fn agents(
        engine: &Engine,
        session: &str,
    ) -> Result<Option<Self>, kuvo_server::Refusal> {
        Ok(engine
            .agent_session(session)
            .await?
            .map(|(source, _)| Self::Agents(source)))
    }

    /// The place to follow a conversation's events on, without starting the
    /// engine: the agents' for theirs, else the engine's if it runs.
    pub(crate) async fn passive(engine: &Engine, session: &str) -> Option<Self> {
        if let Ok(Some(place)) = Self::agents(engine, session).await {
            return Some(place);
        }
        engine.acquire(Demand::Passive).await.map(Self::Engine)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Target<'a> {
    Engine(&'a Lease),
    Agents(&'a dyn AgentSessionSource),
}

/// The agents' errors in the engine's terms, so the queue's checks (a
/// missing message, a refusal) hold for both.
fn answered(error: io::Error) -> api::Error {
    let status = match error.kind() {
        io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
        io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported => StatusCode::BAD_REQUEST,
        io::ErrorKind::AlreadyExists => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    api::Error::Status(status, error.to_string())
}

fn inbox_path(session: &str, id: &str) -> String {
    format!("{}/inbox/{}", sessions::path(session), api::segment(id))
}

impl Target<'_> {
    /// The process a handoff to it depends on: one that ends can't take it.
    pub(crate) fn pid(self) -> u32 {
        match self {
            Self::Engine(lease) => lease.record.pid,
            Self::Agents(_) => std::process::id(),
        }
    }

    pub(crate) async fn inbox(
        self,
        engine: &Engine,
        session: &str,
    ) -> Result<Vec<Inbox>, api::Error> {
        match self {
            Self::Engine(lease) => {
                engine
                    .call(
                        lease,
                        Method::GET,
                        &format!("{}/inbox", sessions::path(session)),
                        None,
                    )
                    .await
            }
            Self::Agents(source) => {
                let rows = source
                    .line(session)
                    .await
                    .map_err(answered)?
                    .ok_or_else(|| answered(io::ErrorKind::NotFound.into()))?;
                rows.into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<_, _>>()
                    .map_err(api::Error::Invalid)
            }
        }
    }

    /// Whether the conversation's transcript has message `id`.
    pub(crate) async fn has_message(
        self,
        engine: &Engine,
        session: &str,
        id: &str,
    ) -> Result<bool, api::Error> {
        match self {
            Self::Engine(lease) => {
                let path = format!("{}/message/{}", sessions::path(session), api::segment(id));
                match engine.call::<Value>(lease, Method::GET, &path, None).await {
                    Ok(message) if message["id"] == id || message["info"]["id"] == id => Ok(true),
                    Err(error) if error.missing() => Ok(false),
                    Err(error) => Err(error),
                    Ok(_) => Err(api::Error::Status(
                        StatusCode::BAD_GATEWAY,
                        "The engine did not confirm the message by ID.".into(),
                    )),
                }
            }
            Self::Agents(source) => source.has_message(session, id).await.map_err(answered),
        }
    }

    pub(crate) async fn set_delivery(
        self,
        engine: &Engine,
        session: &str,
        id: &str,
        delivery: Delivery,
    ) -> Result<(), api::Error> {
        match self {
            Self::Engine(lease) => {
                engine
                    .call::<Value>(
                        lease,
                        Method::PATCH,
                        &inbox_path(session, id),
                        Some(&serde_json::json!({ "delivery": delivery })),
                    )
                    .await?;
                Ok(())
            }
            Self::Agents(source) => source
                .set_delivery(session, id, delivery == Delivery::Queue)
                .await
                .map_err(answered),
        }
    }

    /// Take a waiting item out of the inbox. A missing one is a 404.
    pub(crate) async fn take_back(
        self,
        engine: &Engine,
        session: &str,
        id: &str,
    ) -> Result<(), api::Error> {
        match self {
            Self::Engine(lease) => {
                engine
                    .call::<Value>(lease, Method::DELETE, &inbox_path(session, id), None)
                    .await?;
                Ok(())
            }
            Self::Agents(source) => match source.take_back(session, id).await.map_err(answered)? {
                AgentTakeBack::Removed => Ok(()),
                AgentTakeBack::Missing => Err(api::Error::Status(
                    StatusCode::NOT_FOUND,
                    "That message isn't waiting any more.".into(),
                )),
                AgentTakeBack::Busy => Err(api::Error::Status(
                    StatusCode::CONFLICT,
                    "Kuvo is checking whether the agent took this message.".into(),
                )),
            },
        }
    }

    pub(crate) async fn interrupt(
        self,
        engine: &Engine,
        session: &str,
        resume: bool,
    ) -> Result<(), api::Error> {
        match self {
            Self::Engine(lease) => {
                let path = format!(
                    "{}/interrupt{}",
                    sessions::path(session),
                    if resume { "?resume=true" } else { "" }
                );
                engine
                    .call::<Value>(lease, Method::POST, &path, None)
                    .await?;
                Ok(())
            }
            Self::Agents(source) => source
                .interrupt(session, resume)
                .await
                .map(|_| ())
                .map_err(answered),
        }
    }

    pub(crate) async fn is_running(
        self,
        engine: &Engine,
        session: &str,
    ) -> Result<bool, api::Error> {
        match self {
            Self::Engine(lease) => {
                let active = engine
                    .call::<Value>(lease, Method::GET, "api/session/active", None)
                    .await?;
                Ok(active
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| row == session))
                    || active.get(session).is_some_and(|value| !value.is_null()))
            }
            Self::Agents(source) => Ok(source
                .running_changes()
                .is_some_and(|running| running.borrow().contains(session))),
        }
    }

    /// The conversation's last `limit` rows, newest first.
    pub(crate) async fn recent(
        self,
        engine: &Engine,
        session: &str,
        limit: u32,
    ) -> Result<Vec<Value>, api::Error> {
        match self {
            Self::Engine(lease) => {
                let path = format!(
                    "{}/message?limit={limit}&order=desc",
                    sessions::path(session)
                );
                engine.call(lease, Method::GET, &path, None).await
            }
            Self::Agents(source) => source.recent(session, limit).await.map_err(answered),
        }
    }

    /// Hand the conversation a message, as the engine's `prompt` takes it.
    pub(crate) async fn prompt(
        self,
        engine: &Engine,
        session: &str,
        prompt: &Prompt,
        wait: Duration,
    ) -> Result<Value, api::Error> {
        match self {
            Self::Engine(lease) => {
                let body = serde_json::to_value(prompt).map_err(api::Error::Invalid)?;
                engine
                    .call_raw(
                        lease,
                        Method::POST,
                        &format!("{}/prompt", sessions::path(session)),
                        Some(&body),
                        wait,
                    )
                    .await
            }
            Self::Agents(source) => {
                source
                    .put_in_line(
                        session,
                        AgentLinePrompt {
                            id: prompt.id.clone(),
                            text: prompt.text.clone(),
                            files: prompt.files.clone(),
                            queue: prompt.delivery == Delivery::Queue,
                            metadata: prompt.metadata.as_ref().and_then(Value::as_object).cloned(),
                        },
                    )
                    .await
                    .map_err(answered)?;
                Ok(Value::Object(Map::new()))
            }
        }
    }
}
