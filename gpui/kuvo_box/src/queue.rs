//! A conversation's queue (PROTOCOL.md, "The queue", `queue.items`): every message waiting, in the
//! order the box says clients show them, and what's done with one: taken back (to edit it, or for
//! good), steered into the run under way, or sent now; and the queue resumed after a Stop.

use serde::Deserialize;
use std::time::Duration;

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::follow::{Follow, Followed, follow_lines};
use crate::parts::File;
use crate::paths;
use crate::transport::Request;

/// A session's queue as clients show it.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Queue {
    pub paused: bool,
    /// Every message waiting, in order; none while where it runs can't be read.
    pub items: Option<Vec<Item>>,
    /// Why what waits on the box waits, in its words, while its engine is being updated (PROTOCOL.md,
    /// "A new engine"); none otherwise.
    #[serde(default)]
    pub line: Option<String>,
}

/// One message waiting: your words (without what the box adds for the agent), its files' names, the
/// keyword it went with, who it's from (none for yours), where it waits, and whether Stop held it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Item {
    pub id: String,
    pub words: String,
    #[serde(default)]
    pub files: Vec<String>,
    pub keyword: Option<String>,
    pub from: Option<String>,
    pub on: String,
    pub delivery: String,
    pub held: bool,
}

impl Item {
    /// Yours (not another agent's letter, a wake-up or a note): only yours can be edited.
    pub fn is_yours(&self) -> bool {
        self.from.is_none()
    }

    /// Waiting for the run to end (not for the agent's next step).
    pub fn is_queued(&self) -> bool {
        self.delivery != "steer"
    }

    /// Waiting on the box itself (it came while the queue was paused), not in the engine.
    pub fn can_reorder(&self) -> bool {
        self.held || self.is_on_box() || self.is_queued()
    }

    pub fn is_on_box(&self) -> bool {
        self.on == "box"
    }

    /// Handed to the agent for its next step (steered, in its engine or its coding agent's
    /// bridge): clients draw it in the conversation, not in the queue over the composer.
    pub fn is_for_next_step(&self) -> bool {
        !self.is_queued() && !self.is_on_box() && !self.held
    }
}

/// One taken back: its words as shown, the keyword it went with, what goes back in the composer as
/// its box says (`draft`: its words with the command or keyword in front, a /loop's alone), and its
/// files as the send takes them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Removed {
    pub id: String,
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub draft: String,
    #[serde(default)]
    pub files: Vec<File>,
    /// Everything it went with, for Edit to put back: `files`, then those its box kept for the
    /// agent to open (a box from before has none: `files` is all there is).
    #[serde(default)]
    pub attached: Option<Vec<File>>,
    pub receipt: Option<String>,
}

pub fn route(session: &str) -> String {
    format!("kuvo/sessions/{}/queue", paths::segment(session))
}

impl BoxClient {
    /// The queue with every message waiting (`?items=1`: a client that doesn't draw them isn't
    /// slowed by the engine's being asked).
    pub fn queue(&self, session: &str) -> Result<Queue> {
        self.get(&format!("{}?items=1", route(session)))
    }

    /// Follows `session`'s queue: now, and again as it changes.
    pub fn follow_queue(&self, session: &str) -> Follow<Followed<Queue>> {
        follow_lines(self, format!("{}/events", route(session)))
    }

    /// Steer: `item` to the agent at its next step, the run going on (`queue.steer`; its box says
    /// where the agent can't take one partway, and sends one with nothing running now).
    pub fn steer_queued(&self, session: &str, item: &str) -> Result<()> {
        self.done(&Request::post(format!("{}/{}/steer", route(session), paths::segment(item)), serde_json::json!({})))
    }

    /// Send Now: `item` to the agent right away, the step under way stopped for it.
    pub fn send_queued_now(&self, session: &str, item: &str) -> Result<()> {
        self.done(&Request::post(format!("{}/{}/now", route(session), paths::segment(item)), serde_json::json!({})))
    }

    pub fn reorder_queue(&self, session: &str, item: &str, anchor: &str, before: bool) -> Result<Queue> {
        self.value(&Request::post(format!("{}/reorder", route(session)), serde_json::json!({ "item": item, "anchor": anchor, "before": before })))
    }

    /// Takes `item` out of the queue (wherever it waits), with what it was.
    pub fn remove_queued(&self, session: &str, item: &str) -> Result<Option<Removed>> {
        self.remove_queued_with_receipt(session, item, None, false)
    }

    /// An operation ID lets a subsequent retry recover its own removal across reconnects.
    /// `arriving`: this client still shows it on its way (sent, not listed yet). The box takes it
    /// out as it arrives (`409` meanwhile, asked again), or says it went to the agent already.
    pub fn remove_queued_with_receipt(&self, session: &str, item: &str, receipt: Option<&str>, arriving: bool) -> Result<Option<Removed>> {
        #[derive(Deserialize)]
        struct Answer {
            removed: Option<Removed>,
        }
        let mut path = format!("{}/{}", route(session), paths::segment(item));
        let mut query = Vec::new();
        if let Some(receipt) = receipt {
            query.push(format!("receipt={}", paths::query_value(receipt)));
        }
        if arriving {
            query.push("arriving=1".to_owned());
        }
        if !query.is_empty() {
            path.push_str(&format!("?{}", query.join("&")));
        }
        let request = Request::delete(path);
        let read = || {
            let response = self.send(&request)?;
            if !response.is_success() {
                return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
            }
            serde_json::from_slice::<Answer>(&response.body).map_err(|error| Error::BadAnswer(error.to_string()))
        };
        let mut answer = read();
        for delay in [200, 800] {
            let Err(error) = &answer else { break };
            if !removal_retryable(error) {
                break;
            }
            std::thread::sleep(Duration::from_millis(delay));
            answer = read();
        }
        let answer = answer?;
        Ok(answer.removed)
    }

    /// Release a durable removal receipt once the client has used its draft or discarded it.
    pub fn acknowledge_queued_removal(&self, session: &str, item: &str, receipt: &str) -> Result<()> {
        self.done(&Request::post(format!("{}/{}/ack", route(session), paths::segment(item)), serde_json::json!({ "receipt": receipt })))
    }
}

pub fn removal_retryable(error: &Error) -> bool {
    matches!(error, Error::Unreachable(_) | Error::NotRunning(_)) || matches!(error.status(), Some(409 | 502..=504))
}
