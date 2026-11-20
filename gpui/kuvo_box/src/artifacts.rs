//! The outputs recorded for a conversation, as its artifact list names them.
use serde::Deserialize;

use crate::error::{Error, Refusal, Result};
use crate::follow::{Followed, follow_lines};
use crate::paths::segment;
use crate::transport::Request;
use crate::{BoxClient, Follow, Timing};

#[derive(Clone, Debug, Deserialize)]
pub struct Outputs {
    pub artifacts: Vec<Artifact>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub gone: Option<f64>,
    pub latest: Version,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Version {
    pub n: u32,
    pub hash: Option<String>,
    pub reason: Option<String>,
}

impl BoxClient {
    pub fn session_outputs(&self, session: &str, before: Option<&str>) -> Result<Outputs> {
        let mut route = format!("kuvo/artifacts?session={}", segment(session));
        if let Some(before) = before {
            route.push_str(&format!("&before={}", segment(before)));
        }
        self.get(&route)
    }

    pub fn follow_session_outputs(&self, session: &str, timing: Timing) -> Follow<Followed<Outputs>> {
        let id = segment(session);
        follow_lines(self, "session outputs", format!("kuvo/artifacts/events?session={id}"), format!("kuvo/artifacts?session={id}"), timing)
    }

    pub fn artifact_bytes(&self, artifact: &Artifact) -> Result<Vec<u8>> {
        let id = segment(&artifact.id);
        let n = artifact.latest.n;
        let preview = if artifact.kind == "image" { "?preview=1" } else { "" };
        let response = self.send(&Request::get(format!("kuvo/artifacts/{id}/versions/{n}{preview}")))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(Error::Refused(Refusal::from_answer(response.status, &response.body)))
        }
    }
}
