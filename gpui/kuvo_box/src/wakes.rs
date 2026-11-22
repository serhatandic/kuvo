//! A bot conversation's schedule, served and followed on the computer it lives on.

use serde::{Deserialize, Serialize};

use crate::{
    BoxClient, Follow,
    error::Result,
    follow::{Followed, follow_lines},
    paths,
    transport::Request,
};

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Schedule {
    pub wakes: Vec<Wake>,
    pub watches: Vec<Watch>,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wake {
    pub id: String,
    pub due: f64,
    pub every: Option<String>,
    pub summary: String,
    pub cancel_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Watch {
    pub letters: String,
    pub next: Option<f64>,
    pub every: String,
    pub summary: String,
}

/// Stable keys from the schedule, including when a repeating wake moves to its next time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cancel {
    Wake(String),
    Watch(String),
}

fn route(session: &str) -> String {
    format!("kuvo/wakes/{}", paths::segment(session))
}

impl BoxClient {
    pub fn wake_schedule(&self, session: &str) -> Result<Schedule> {
        self.get(&route(session))
    }

    pub fn follow_wake_schedule(
        &self,
        session: &str,
    ) -> Follow<Followed<Schedule>> {
        let path = route(session);
        follow_lines(self, format!("{path}/events"))
    }

    pub fn cancel_wake(&self, session: &str, cancel: &Cancel) -> Result<()> {
        self.done(&Request::post(
            format!("{}/cancel", route(session)),
            serde_json::json!(cancel),
        ))
    }
}
