//! Read saved agent conversations through the resident.

use http::StatusCode;
use kuvo_agents::{
    session_runs::SessionRuns,
    session_store::{MessageOrder, SessionStore},
};
use kuvo_agents::{
    accounts::Provider,
    live_asks::AskKey,
    resident::ResidentAgents,
    session::{Prompt, TurnKind},
    session_store::{NewSession, SessionPatch},
    session_view,
};
use kuvo_engine::{
    AgentCall, AgentPrompt, AgentSessionPatch, AgentSessionRead, AgentSessionRow,
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
use tokio::sync::watch;

pub(crate) struct Inventory {
    pub store: SessionStore,
    pub running: Option<SessionRuns>,
    pub home: std::path::PathBuf,
    pub agents: Option<ResidentAgents>,
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
            Ok(self
                .store
                .read(id)
                .await?
                .map(|saved| session_view::for_clients(saved.into_fields(), &self.home)))
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
            let saved = self
                .store
                .read(id)
                .await?
                .ok_or(io::ErrorKind::NotFound)?;
            let folder = saved
                .directory()
                .map(std::path::PathBuf::from)
                .ok_or(io::ErrorKind::InvalidData)?;
            let AgentPrompt { id: message, text, metadata } = prompt;
            let (input, policy) = super::turns::turn_setup(
                &self.home,
                folder,
                &saved.into_fields(),
                &message,
                TurnKind::Prompt(Prompt {
                    id: message.clone(),
                    text,
                    files: Vec::new(),
                    synthetic: false,
                }),
            )
            .map_err(|refusal| io::Error::other(format!("{:?}", refusal.reply().status)))?;
            runs.run_turn_noted(id.to_owned(), input, policy, metadata)
                .await
                .map(|_| ())
        })
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
