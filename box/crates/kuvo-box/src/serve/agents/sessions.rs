//! Read saved agent conversations through the resident.

use http::StatusCode;
use kuvo_agents::{
    session_runs::{Cancelled, SessionRuns},
    session_store::{MessageOrder, SessionStore},
};
use kuvo_agents::{
    accounts::Provider,
    live_asks::AskKey,
    resident::ResidentAgents,
    inbox::{Delivery, Input},
    session_store::{NewSession, SessionPatch},
    session_view,
};
use kuvo_engine::{
    AgentCall, AgentEvent, AgentLinePrompt, AgentPrompt, AgentTakeBack, AgentSessionPatch, AgentSessionRead, AgentSessionRow,
    AgentSessionSource, NewAgentSession,
};
use std::io;
use serde_json::{Map, Value};
use kuvo_protocol::routes::{
    get_kuvo_agents_api_session_id_message as transcript,
    types::GetKuvoAgentsApiSessionIdMessageStatus200Cursor as Cursor,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use percent_encoding::percent_decode_str;
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::{broadcast, watch};

pub(crate) struct Inventory {
    pub store: SessionStore,
    pub running: Option<SessionRuns>,
    pub home: std::path::PathBuf,
    pub agents: Option<ResidentAgents>,
}

impl Inventory {
    /// The window its agent lists for the conversation's model (not one it's still guessing).
    async fn listed_window(&self, fields: &Map<String, Value>) -> Option<u64> {
        let model = fields.get("contextModel").or_else(|| fields.get("model"))?;
        let provider = Provider::parse(model["providerID"].as_str()?)?;
        let id = model["id"].as_str()?;
        let roster = self.agents.as_ref()?.roster();
        let models = roster.find(&provider)?.agent.models(false).await.ok()?;
        models
            .models
            .iter()
            .find(|each| {
                !each.provisional && (each.id == id || each.aliases.iter().any(|alias| alias == id))
            })?
            .context
            .filter(|window| *window > 0)
    }

    fn runs(&self) -> io::Result<&SessionRuns> {
        self.running
            .as_ref()
            .ok_or_else(|| io::Error::other("The coding agents aren't running here."))
    }
}

fn no_model() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Pick a model of an agent you've connected.",
    )
}

impl AgentSessionSource for Inventory {
    fn inventory(&self) -> AgentSessionRead<'_> {
        Box::pin(async move {
            let active = self.running.as_ref().map(SessionRuns::running);
            Ok(self
                .store
                .headers()
                .await?
                .into_iter()
                .map(|saved| {
                    let running = active
                        .as_ref()
                        .is_some_and(|active| active.contains(saved.id()));
                    AgentSessionRow {
                        header: saved.into_fields(),
                        running,
                    }
                })
                .collect())
        })
    }

    fn running_changes(&self) -> Option<watch::Receiver<Arc<BTreeSet<String>>>> {
        self.running.as_ref().map(SessionRuns::changes)
    }

    fn session<'a>(&'a self, id: &'a str) -> AgentCall<'a, Option<Map<String, Value>>> {
        Box::pin(async move {
            let Some(saved) = self.store.read(id).await? else {
                return Ok(None);
            };
            let mut fields = saved.into_fields();
            // How much its model holds, as its agent's list says, until a turn says.
            if !fields.contains_key("contextWindow")
                && let Some(window) = self.listed_window(&fields).await
            {
                fields.insert("contextWindow".into(), window.into());
                if let Some(context) = fields.get_mut("context").and_then(Value::as_object_mut) {
                    context.entry("window").or_insert(window.into());
                }
            }
            Ok(Some(session_view::for_clients(fields, &self.home)))
        })
    }

    fn update<'a>(
        &'a self,
        id: &'a str,
        patch: AgentSessionPatch,
    ) -> AgentCall<'a, Option<Map<String, Value>>> {
        Box::pin(async move {
            let AgentSessionPatch {
                title,
                permissions,
                metadata,
            } = patch;
            self.store
                .update(
                    id,
                    SessionPatch {
                        title,
                        permissions,
                        metadata,
                    },
                )
                .await
        })
    }

    fn delete<'a>(&'a self, id: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async move {
            match &self.running {
                Some(runs) => runs.delete(id.to_owned()).await,
                None => self.store.delete(id).await,
            }
        })
    }

    fn create(&self, new: NewAgentSession) -> AgentCall<'_, Map<String, Value>> {
        Box::pin(async move {
            let provider = new.model["providerID"]
                .as_str()
                .and_then(Provider::parse)
                .filter(|_| new.model["id"].as_str().is_some_and(|id| !id.is_empty()))
                .ok_or_else(no_model)?;
            let agents = self.agents.as_ref().ok_or_else(no_model)?;
            if agents.roster().find(&provider).is_none() {
                return Err(no_model());
            }
            let NewAgentSession {
                id,
                directory,
                agent,
                model,
                title,
                permissions,
                metadata,
            } = new;
            self.store
                .begin(NewSession {
                    id,
                    directory,
                    kind: provider.kind,
                    agent,
                    model,
                    title,
                    permissions,
                    metadata,
                })
                .await
        })
    }

    fn prompt<'a>(&'a self, id: &'a str, prompt: AgentPrompt) -> AgentCall<'a, ()> {
        Box::pin(async move {
            let runs = self
                .running
                .as_ref()
                .ok_or_else(|| io::Error::other("The coding agents aren't running here."))?;
            let AgentPrompt { id: message, text, metadata } = prompt;
            let mut input = Input::new(message, text, Vec::new(), Delivery::Steer);
            input.metadata = metadata;
            runs.prompt(id.to_owned(), input, None).await
        })
    }

    fn line<'a>(&'a self, id: &'a str) -> AgentCall<'a, Option<Vec<Value>>> {
        Box::pin(async move {
            Ok(self.store.line(id).await?.map(|line| {
                line.iter()
                    .map(|input| input.listed(id))
                    .collect()
            }))
        })
    }

    fn put_in_line<'a>(&'a self, id: &'a str, prompt: AgentLinePrompt) -> AgentCall<'a, ()> {
        Box::pin(async move {
            let runs = self.runs()?;
            let AgentLinePrompt {
                id: message,
                text,
                files,
                queue,
                metadata,
            } = prompt;
            let delivery = if queue { Delivery::Queue } else { Delivery::Steer };
            let mut input = Input::new(message, text, files, delivery);
            input.metadata = metadata;
            runs.prompt(id.to_owned(), input, None).await
        })
    }

    fn set_delivery<'a>(&'a self, id: &'a str, item: &'a str, queue: bool) -> AgentCall<'a, ()> {
        Box::pin(async move {
            let delivery = if queue { Delivery::Queue } else { Delivery::Steer };
            self.runs()?
                .set_delivery(id.to_owned(), item.to_owned(), delivery, false)
                .await
        })
    }

    fn take_back<'a>(&'a self, id: &'a str, item: &'a str) -> AgentCall<'a, AgentTakeBack> {
        Box::pin(async move {
            Ok(
                match self
                    .runs()?
                    .cancel_input(id.to_owned(), item.to_owned(), false)
                    .await?
                {
                    Cancelled::Removed => AgentTakeBack::Removed,
                    Cancelled::Missing => AgentTakeBack::Missing,
                    Cancelled::Busy => AgentTakeBack::Busy,
                },
            )
        })
    }

    fn interrupt<'a>(&'a self, id: &'a str, resume: bool) -> AgentCall<'a, bool> {
        Box::pin(async move { self.runs()?.interrupt_and(id.to_owned(), resume).await })
    }

    fn has_message<'a>(&'a self, id: &'a str, message: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async move { self.store.has_message(id, message).await })
    }

    fn recent<'a>(&'a self, id: &'a str, limit: u32) -> AgentCall<'a, Vec<Value>> {
        Box::pin(async move { self.runs()?.recent(id, limit).await })
    }

    fn events(&self) -> Option<broadcast::Receiver<AgentEvent>> {
        let mut line = self.running.as_ref()?.events();
        let (words, heard) = broadcast::channel(256);
        tokio::spawn(async move {
            loop {
                match line.recv().await {
                    Ok(event) => {
                        let (session, kind, data) = event.engine_words();
                        let _ = words.send(AgentEvent {
                            session: session.to_owned(),
                            kind,
                            data,
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Some(heard)
    }

    fn stop_if_waiting<'a>(&'a self, id: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async move {
            let Some(runs) = &self.running else {
                return Ok(false);
            };
            let waiting = runs
                .asks()
                .pending()
                .borrow()
                .keys()
                .any(|AskKey { session, .. }| session == id);
            if waiting {
                runs.interrupt(id.to_owned()).await?;
            }
            Ok(waiting)
        })
    }
}

pub(crate) fn register(sessions: SessionStore, routes: &mut Registry) {
    routes.register::<transcript::Spec, _, _>(Access::Any, move |call| {
        let sessions = sessions.clone();
        async move {
            let given = call.param("id").unwrap_or_default();
            let id = percent_decode_str(given)
                .decode_utf8()
                .map_err(|_| missing(given))?;
            let query = TranscriptQuery::parse(&call.request.path);
            let page = sessions
                .message_page(&id, query.after, query.limit, query.order)
                .await
                .map_err(|error| {
                    eprintln!("Kuvo could not read the coding-agent transcript {id}: {error}");
                    Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "The conversation couldn't be read. Try again in a moment.",
                    )
                })?
                .ok_or_else(|| missing(&id))?;
            let next = page
                .messages
                .last()
                .and_then(|message| message["id"].as_str())
                .filter(|_| page.more)
                .map(|last| {
                    let order = &query.cursor_order;
                    match query.filter.filter(|filter| !filter.is_empty()) {
                        Some(filter) => format!("{order}:{last}:{filter}"),
                        None => format!("{order}:{last}"),
                    }
                });
            Ok(RouteReply::json::<200>(transcript::Status200 {
                data: page.messages,
                cursor: Cursor { next },
            }))
        }
    });
}

pub(super) fn missing(id: &str) -> Refusal {
    Refusal::new(StatusCode::NOT_FOUND, format!("No session {id}."))
}

struct TranscriptQuery {
    after: Option<String>,
    limit: u32,
    order: MessageOrder,
    cursor_order: String,
    filter: Option<String>,
}

impl TranscriptQuery {
    fn parse(path: &str) -> Self {
        let query = path.split_once('?').map_or("", |(_, query)| query);
        let get = |key| {
            query
                .split('&')
                .filter_map(|part| part.split_once('='))
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| {
                    percent_decode_str(&value.replace('+', " "))
                        .decode_utf8()
                        .ok()
                        .map(|value| value.into_owned())
                })
        };
        let cursor = get("cursor").filter(|value| !value.is_empty());
        let mut parts = cursor
            .as_deref()
            .into_iter()
            .flat_map(|text| text.split(':'));
        let from_cursor = parts.next();
        let after = cursor.as_ref().map(|_| parts.next().unwrap_or("").into());
        let given_order = get("order");
        let oldest = given_order.as_deref() == Some("asc")
            || from_cursor.is_some_and(|text| text.starts_with("asc"));
        let limit = get("limit")
            .and_then(|limit| limit.trim().parse::<u32>().ok())
            .filter(|limit| *limit != 0)
            .unwrap_or(100);
        Self {
            after,
            limit,
            order: if oldest {
                MessageOrder::OldestFirst
            } else {
                MessageOrder::NewestFirst
            },
            cursor_order: from_cursor
                .map(str::to_owned)
                .or(given_order)
                .unwrap_or_else(|| "desc".into()),
            filter: get("parentID").or_else(|| parts.next().map(str::to_owned)),
        }
    }
}
