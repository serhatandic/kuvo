//! The engine's side of wake-ups and watches. The resident's wake-up rounds and Wake Up Now reach
//! a conversation through this handle, as `kuvo-wake fire` reached the engine over HTTP; the
//! handle keeps the engine up for as long as it's held.

use std::time::{Duration, Instant};

use http::{HeaderMap, HeaderValue, Method};
use serde_json::{Value, json};

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
    queue::{self, Delivery, Prompt},
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

/// The engine, held up for a round of wake-ups.
pub struct WakeEngine {
    engine: Engine,
    lease: Lease,
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
            lease,
        })
    }
}

impl WakeEngine {
    /// The session as the engine has it now; `None` once it's gone.
    pub async fn session(&self, session: &str) -> Result<Option<Value>, Unreachable> {
        if !sessions::valid_id(session) {
            return Ok(None);
        }
        match self
            .engine
            .call_raw(
                &self.lease,
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
                    &self.lease,
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
        crate::asks::stop_waiting(&self.engine, &self.lease, session).await;
    }

    /// Whether the conversation's queue is stopped (you stopped it): what arrives waits on the box.
    pub async fn stopped(&self, session: &str) -> bool {
        self.engine
            .queues
            .get(session)
            .await
            .is_ok_and(|queue| queue.is_paused())
    }

    /// Whether this wake-up's note waits in the engine's own inbox for the conversation still.
    pub async fn waits_in_inbox(&self, session: &str, note: &str) -> bool {
        let text = format!("{WAKE_MARKER}{note}");
        queue::inbox(&self.engine, &self.lease, session)
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
        let id = crate::send::new_id(&self.engine).map_err(|_| Unreachable("No ID".into()))?;
        let _turn = self.engine.turn(session).await;
        let _admitted = crate::session_move::admit(&self.engine, session, None)
            .await
            .map_err(|_| Unreachable("The conversation is moving.".into()))?;
        let prompt = Prompt {
            id,
            text: format!("{WAKE_MARKER}{note}"),
            delivery: Delivery::Queue,
            files: Vec::new(),
            metadata: summary
                .filter(|summary| !summary.is_empty())
                .map(|summary| json!({ "kuvo": { "wake": { "summary": summary } } })),
        };
        if self
            .engine
            .queues
            .keep(
                session,
                &prompt,
                Some(note),
                Some(WAKE_FROM.to_owned()),
                None,
                &[],
            )
            .await
            .map_err(|error| Unreachable(error.to_string()))?
        {
            self.engine.queues.notify();
            return Ok(Delivered::Held);
        }
        crate::send::post(&self.engine, &self.lease, session, &prompt)
            .await
            .map_err(|_| Unreachable("The wake-up couldn’t be sent.".into()))?;
        self.engine.queues.notify();
        Ok(Delivered::Sent)
    }
}
