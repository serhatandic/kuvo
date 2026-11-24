//! The resident owns each wait until a client answers or the tool goes away.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::{
    Error, Result,
    helper::millis,
    items::{new_id, one_line},
    session_asks::VaultWaiting,
};

const PATIENCE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Question {
    pub id: String,
    pub at: u64,
    pub kind: AskKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip)]
    pub caller_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub always: bool,
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum AskKind {
    #[default]
    Use,
    Item,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Decision {
    Once,
    Always,
    #[default]
    No,
}

#[derive(Default)]
pub(super) struct Answer {
    pub decision: Decision,
    pub item: Option<String>,
    pub by: Option<String>,
}

pub(super) struct Book {
    open: Mutex<BTreeMap<String, Open>>,
    changes: watch::Sender<Vec<Question>>,
    waiting: watch::Sender<Arc<Vec<VaultWaiting>>>,
}

struct Open {
    question: Question,
    sender: oneshot::Sender<Answer>,
}

struct Waiting {
    book: Arc<Book>,
    id: String,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.book.take(&self.id);
    }
}

impl Default for Book {
    fn default() -> Self {
        Self {
            open: Mutex::default(),
            changes: watch::channel(Vec::new()).0,
            waiting: watch::channel(Arc::new(Vec::new())).0,
        }
    }
}

impl Book {
    pub fn list(&self) -> Vec<Question> {
        self.changes.borrow().clone()
    }

    pub fn events(&self) -> watch::Receiver<Vec<Question>> {
        self.changes.subscribe()
    }

    pub fn waiting(&self) -> watch::Receiver<Arc<Vec<VaultWaiting>>> {
        self.waiting.subscribe()
    }

    pub async fn ask(
        self: &Arc<Self>,
        mut question: Question,
        cancel: &CancellationToken,
    ) -> Result<Answer> {
        if cancel.is_cancelled() {
            return Ok(Answer::default());
        }
        question.id = new_id("q_")?;
        question.at = millis();
        for value in [&mut question.why, &mut question.need] {
            if let Some(value) = value {
                *value = one_line(value, 300);
            }
        }
        let waiting = Waiting {
            book: self.clone(),
            id: question.id.clone(),
        };
        let (sender, receiver) = oneshot::channel();
        {
            let mut open = self
                .open
                .lock()
                .map_err(|_| Error::refused(503, "The vault's asks stopped."))?;
            open.insert(question.id.clone(), Open { question, sender });
            self.publish(&open);
        }
        let answer = tokio::select! {
            value = receiver => value.unwrap_or_default(),
            _ = cancel.cancelled() => Answer::default(),
            _ = tokio::time::sleep(PATIENCE) => Answer::default(),
        };
        drop(waiting);
        Ok(answer)
    }

    pub fn answer(&self, id: &str, answer: Answer) -> bool {
        self.take_matching(id, None)
            .is_some_and(|open| open.sender.send(answer).is_ok())
    }

    pub fn answer_for(&self, session: &str, id: &str, answer: Answer) -> bool {
        self.take_matching(id, Some(session))
            .is_some_and(|open| open.sender.send(answer).is_ok())
    }

    pub fn cancel_for(&self, session: &str, id: &str) -> bool {
        self.take_matching(id, Some(session)).is_some()
    }

    fn take(&self, id: &str) -> Option<Open> {
        self.take_matching(id, None)
    }

    fn take_matching(&self, id: &str, session: Option<&str>) -> Option<Open> {
        let mut open = self.open.lock().ok()?;
        if session.is_some_and(|session| {
            open.get(id).and_then(|ask| ask.question.session.as_deref()) != Some(session)
        }) {
            return None;
        }
        let found = open.remove(id)?;
        self.publish(&open);
        Some(found)
    }

    fn publish(&self, open: &BTreeMap<String, Open>) {
        let mut list: Vec<_> = open.values().map(|open| open.question.clone()).collect();
        list.sort_by_key(|question| question.at);
        self.waiting.send_replace(Arc::new(
            list.iter()
                .filter_map(|ask| {
                    Some(VaultWaiting {
                        id: ask.id.clone(),
                        session: ask.session.clone()?,
                        at: ask.at,
                    })
                })
                .collect(),
        ));
        self.changes.send_replace(list);
    }
}
