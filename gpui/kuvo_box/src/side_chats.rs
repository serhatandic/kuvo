//! A conversation's side chats (PROTOCOL.md, "Side questions",
//! `btw.threads.events`): questions asked aside, answered from what the conversation knows and kept
//! out of it, each side thread with its questions, as the box keeps and groups them; asked on the
//! box the conversation lives on, through its agents bridge.

use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Refusal, Result};
use crate::follow::{Followed, follow_lines};
use crate::parts::File;
use crate::paths;
use crate::transport::Request;
use crate::typed::ModelRef;
use crate::{BoxClient, Follow};

pub const SIDE_CHATS: &str = "btw.threads.events";

/// How far a question's come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answering {
    Answering,
    Answered,
    Failed,
    #[serde(other)]
    Unknown,
}

/// A question asked aside, and its answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SideQuestion {
    pub id: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub question: String,
    /// The model it was asked on, where that isn't the conversation's own.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// The files that went with it, by name.
    #[serde(default)]
    pub files: Vec<String>,
    pub state: Answering,
    #[serde(default)]
    pub answer: Option<String>,
    /// Why it couldn't be answered, in plain words.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub asked: f64,
}

/// A side thread: its first question as its title, and its questions, oldest first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SideThread {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub asked: f64,
    #[serde(default)]
    pub latest: f64,
    /// A question in it is still being answered.
    #[serde(default)]
    pub answering: bool,
    #[serde(default)]
    pub questions: Vec<SideQuestion>,
}

/// A conversation's side threads, the latest first.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct SideThreads {
    #[serde(default)]
    pub threads: Vec<SideThread>,
}

impl SideThreads {
    pub fn thread(&self, id: &str) -> Option<&SideThread> {
        self.threads.iter().find(|thread| thread.id == id)
    }
}

fn route(session: &str) -> String {
    format!("kuvo/agents/api/kuvo/btw/{}", paths::segment(session))
}

impl BoxClient {
    /// `session`'s side threads now.
    pub fn side_threads(&self, session: &str) -> Result<SideThreads> {
        self.get(&format!("{}/threads", route(session)))
    }

    /// `session`'s side threads, now and as each question is asked and answered.
    pub fn follow_side_threads(&self, session: &str) -> Follow<Followed<SideThreads>> {
        let once = format!("{}/threads", route(session));
        follow_lines(self, format!("{once}/events"))
    }

    /// Asks `question` aside about `session`: a follow-up in `thread`, or a new side thread; on
    /// `model` (with its effort) where it's given, else the conversation's own; with `files` (text
    /// and pictures; the box says why it refuses one it can't take). Answers the question as the box keeps it (being
    /// answered).
    pub fn ask_aside(&self, session: &str, question: &str, thread: Option<&str>, model: Option<&ModelRef>, files: &[File]) -> Result<SideQuestion> {
        #[derive(Deserialize)]
        struct Answer {
            data: SideQuestion,
        }
        let mut body = json!({ "question": question });
        if let Some(thread) = thread {
            body["thread"] = json!(thread);
        }
        if let Some(model) = model {
            body["model"] = json!(model);
        }
        if !files.is_empty() {
            body["files"] = json!(files);
        }
        let response = self.send(&Request::post(route(session), body))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        serde_json::from_slice::<Answer>(&response.body).map(|answer| answer.data).map_err(|error| Error::BadAnswer(format!("The answer couldn't be read: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_threads_read_with_their_questions_as_the_box_groups_them() {
        let read: SideThreads = serde_json::from_str(r#"{"threads":[{"id":"q2","title":"Should 409 share the bucket?","asked":3,"latest":4,"answering":true,"questions":[{"id":"q2","thread":"q2","question":"Should 409 share the bucket?","state":"answering","asked":3}]},{"id":"q1","title":"Why 23505?","asked":1,"latest":2,"answering":false,"questions":[{"id":"q1","question":"Why 23505?","state":"answered","answer":"It's atomic.","asked":1}]}]}"#).unwrap();
        assert!(read.threads[0].answering);
        assert_eq!(read.thread("q1").unwrap().questions[0].answer.as_deref(), Some("It's atomic."));
        assert_eq!(route("ses_1"), "kuvo/agents/api/kuvo/btw/ses_1");
    }
}
