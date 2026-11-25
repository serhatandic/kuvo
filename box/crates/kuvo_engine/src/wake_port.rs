//! The engine's side of wake-ups and watches. The resident's wake-up rounds and Wake Up Now reach
//! a conversation through this handle, as `kuvo-wake fire` reached the engine over HTTP; the
//! handle keeps the engine up for as long as it's held.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use http::{HeaderMap, HeaderValue, Method};
use serde_json::{Value, json};

use crate::{
    AgentSessionSource, Engine, api,
    lifetime::{Demand, Lease},
    queue::{self, Delivery, Prompt, Target},
    sessions,
};

/// What a wake-up's message starts with, as the Mac words it.
pub const WAKE_MARKER: &str = "[Wake-up] ";
/// Who a wake-up waiting on the box is from, as its queue shows it.
pub const WAKE_FROM: &str = "Wake-up";
const MCP_WAIT: Duration = Duration::from_secs(20);
const CALL_WAIT: Duration = Duration::from_secs(5);

/// Why the engine couldn't be asked or written to now; a later round tries again.
#[derive(Debug)]
pub struct Unreachable(pub String);

impl std::fmt::Display for Unreachable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Unreachable {}

impl From<api::Error> for Unreachable {
    fn from(error: api::Error) -> Self {
        Self(error.to_string())
    }
}

/// How a wake-up went: sent to the conversation, or kept waiting on the box with its stopped
/// queue (it goes once you let the conversation go on).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivered {
    Sent,
    Held,
}

/// A message the box writes itself: its id, its text and mark, who it's shown from and in what
/// words while it waits on the box, and whether it's read at the conversation's next step (`steer`)
/// rather than after its turn.
pub struct BoxMessage<'a> {
    pub id: String,
    pub text: String,
    pub metadata: Option<Value>,
    pub from: &'a str,
    pub words: &'a str,
    pub steer: bool,
}

/// The engine, held up for a round of wake-ups, and the coding agents for theirs; or the coding
/// agents alone, for a round only they have anything in.
pub struct WakeEngine {
    engine: Engine,
    lease: Option<Lease>,
}

impl Engine {
    /// The engine for wake-ups: started when `start` (one is due), else only one already running.
    pub async fn wake_engine(&self, start: bool) -> Option<WakeEngine> {
        let lease = self
            .acquire(if start {
                Demand::Start
            } else {
                Demand::Existing
            })
            .await?;
        Some(WakeEngine {
            engine: self.clone(),
            lease: Some(lease),
        })
    }

    /// The coding agents alone for wake-ups, the engine left as it is; none where they don't run
    /// here.
    pub fn agents_wake_engine(&self) -> Option<WakeEngine> {
        self.agent_sessions.as_ref()?;
        Some(WakeEngine {
            engine: self.clone(),
            lease: None,
        })
    }
}

impl WakeEngine {
    /// Whether this round reaches the coding agents' conversations (`agents`), or the engine's.
    pub fn reaches(&self, agents: bool) -> bool {
        if agents {
            self.engine.agent_sessions.is_some()
        } else {
            self.lease.is_some()
        }
    }

    fn lease(&self) -> Result<&Lease, Unreachable> {
        let down = || Unreachable("The engine isn't running.".into());
        self.lease.as_ref().ok_or_else(down)
    }

    /// The coding agents, where the conversation is theirs.
    async fn theirs(
        &self,
        session: &str,
    ) -> Result<Option<Arc<dyn AgentSessionSource>>, Unreachable> {
        let down = |_| Unreachable("The coding agents on this computer aren't answering.".into());
        Ok(self
            .engine
            .agent_session(session)
            .await
            .map_err(down)?
            .map(|(source, _)| source))
    }

    /// The session as the engine, or its coding agents, have it now; `None` once it's gone.
    pub async fn session(&self, session: &str) -> Result<Option<Value>, Unreachable> {
        let down = |_| Unreachable("The coding agents on this computer aren't answering.".into());
        if let Some((_, header)) = self.engine.agent_session(session).await.map_err(down)? {
            return Ok(Some(Value::Object(header)));
        }
        if !sessions::valid_id(session) {
            return Ok(None);
        }
        match self
            .engine
            .call_raw(
                self.lease()?,
                Method::GET,
                &sessions::path(session),
                None,
                CALL_WAIT,
            )
            .await
        {
            Ok(mut value) => {
                if let Some(data) = value.get_mut("data").filter(|data| data.is_object()) {
                    value = data.take();
                }
                Ok(Some(value))
            }
            Err(error) if error.missing() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Until the MCP servers of the folder a conversation works in are connected, twenty seconds
    /// at most, as the engine's watchdog waits before a wake-up: one sent sooner reaches a model
    /// without their tools. A folder it has only just opened lists none for a moment first.
    pub async fn wait_for_tools(&self, directory: &str) {
        let Some(lease) = &self.lease else {
            return;
        };
        let started = Instant::now();
        let mut headers = HeaderMap::new();
        let Ok(folder) = HeaderValue::from_str(&api::segment(directory)) else {
            return;
        };
        headers.insert("x-opencode-directory", folder);
        let mut pended = false;
        while started.elapsed() < MCP_WAIT {
            let Ok(listed) = self
                .engine
                .call_with_headers(
                    lease,
                    Method::GET,
                    "api/mcp",
                    None,
                    CALL_WAIT,
                    headers.clone(),
                )
                .await
            else {
                return;
            };
            let listed = listed.to_string();
            let none = !listed.contains("\"status\"");
            if listed.contains("\"pending\"") {
                pended = true;
            } else if !none || started.elapsed() >= Duration::from_secs(3) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(if none { 100 } else { 250 })).await;
        }
        if pended {
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    /// A conversation archived while it waits on you stops, as a phone's Archive has it.
    pub async fn stop_if_waiting(&self, session: &str) {
        if let Some(lease) = &self.lease {
            crate::asks::stop_waiting(&self.engine, lease, session).await;
        }
    }

    /// Whether the conversation's queue is stopped (you stopped it): what arrives waits on the box.
    pub async fn stopped(&self, session: &str) -> bool {
        self.engine
            .queues
            .get(session)
            .await
            .is_ok_and(|queue| queue.is_paused())
    }

    /// Whether this wake-up's note waits in the conversation's own inbox still (the engine's, or
    /// the line its coding agent keeps).
    pub async fn waits_in_inbox(&self, session: &str, note: &str) -> bool {
        let text = format!("{WAKE_MARKER}{note}");
        let Ok(theirs) = self.theirs(session).await else {
            return false;
        };
        let target = match (&theirs, &self.lease) {
            (Some(source), _) => Target::Agents(source.as_ref()),
            (None, Some(lease)) => Target::Engine(lease),
            (None, None) => return false,
        };
        queue::inbox(&self.engine, target, session)
            .await
            .is_ok_and(|inbox| inbox.iter().any(|item| item.text_is(&text)))
    }

    /// Whether this wake-up's note waits on the box already, with the conversation's stopped queue.
    pub async fn waits_on_box(&self, session: &str, note: &str) -> bool {
        self.engine
            .queues
            .get(session)
            .await
            .is_ok_and(|queue| queue.waits_from(WAKE_FROM, note))
    }

    /// Sends a wake-up's note to its conversation, marked with its summary (what its card shows),
    /// as one message the box makes; kept waiting on the box while you stopped the conversation.
    pub async fn deliver(
        &self,
        session: &str,
        note: &str,
        summary: Option<&str>,
    ) -> Result<Delivered, Unreachable> {
        let metadata = summary
            .filter(|summary| !summary.is_empty())
            .map(|summary| json!({ "kuvo": { "wake": { "summary": summary } } }));
        let (id, text) = (self.message_id()?, format!("{WAKE_MARKER}{note}"));
        let message = BoxMessage {
            id,
            text,
            metadata,
            from: WAKE_FROM,
            words: note,
            steer: false,
        };
        self.tell(session, message).await
    }

    /// A new message's id, in the engine's order.
    pub fn message_id(&self) -> Result<String, Unreachable> {
        crate::send::new_id(&self.engine).map_err(|_| Unreachable("No ID".into()))
    }

    /// Sends the box's own message to a conversation, waiting on the box (shown from `from`, in
    /// `words`) while you stopped it; never while it moves to or from another computer. One sent
    /// again under the same id is taken once.
    pub async fn tell(
        &self,
        session: &str,
        message: BoxMessage<'_>,
    ) -> Result<Delivered, Unreachable> {
        let BoxMessage {
            id,
            text,
            metadata,
            from,
            words,
            steer,
        } = message;
        let _turn = self.engine.turn(session).await;
        let _admitted = crate::session_move::admit(&self.engine, session, None)
            .await
            .map_err(|_| Unreachable("The conversation is moving.".into()))?;
        let prompt = Prompt {
            id,
            text,
            delivery: if steer {
                Delivery::Steer
            } else {
                Delivery::Queue
            },
            files: Vec::new(),
            metadata,
        };
        let queues = &self.engine.queues;
        let held = queues
            .keep(
                session,
                &prompt,
                Some(words),
                Some(from.to_owned()),
                None,
                true,
            )
            .await
            .map_err(|error| Unreachable(error.to_string()))?;
        if !held {
            let theirs = self.theirs(session).await?;
            let target = match &theirs {
                Some(source) => Target::Agents(source.as_ref()),
                None => Target::Engine(self.lease()?),
            };
            crate::send::post(&self.engine, target, session, &prompt)
                .await
                .map_err(|_| Unreachable("The message couldn’t be sent.".into()))?;
        }
        queues.notify();
        Ok(if held {
            Delivered::Held
        } else {
            Delivered::Sent
        })
    }
}
