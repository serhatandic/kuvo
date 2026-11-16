//! A conversation's queue (PROTOCOL.md, "The queue", `queue.items`): every message waiting, in the
//! order the box says clients show them, and what's done with one: taken back (to edit it, or for
//! good), steered into the run under way, or sent now; and the queue resumed after a Stop.

use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::follow::{Follow, Followed, Timing, follow_lines};
use crate::parts::File;
use crate::paths;
use crate::transport::Request;

/// A session's queue as clients show it.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Queue {
    pub paused: bool,
    /// Every message waiting, in order; none while where it runs can't be read.
    pub items: Option<Vec<Item>>,
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
    pub fn is_on_box(&self) -> bool {
        self.on == "box"
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
    pub fn follow_queue(&self, session: &str, timing: Timing) -> Follow<Followed<Queue>> {
        follow_lines(self, "kuvo-box queue", format!("{}/events", route(session)), format!("{}?items=1", route(session)), timing)
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

    /// Takes `item` out of the queue (wherever it waits), with what it was.
    pub fn remove_queued(&self, session: &str, item: &str) -> Result<Option<Removed>> {
        #[derive(Deserialize)]
        struct Answer {
            removed: Option<Removed>,
        }
        let response = self.send(&Request::delete(format!("{}/{}", route(session), paths::segment(item))))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        let answer: Answer = serde_json::from_slice(&response.body).map_err(|error| Error::BadAnswer(error.to_string()))?;
        Ok(answer.removed)
    }
}
