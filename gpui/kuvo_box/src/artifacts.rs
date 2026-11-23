//! The outputs recorded for a conversation, as its artifact list names them.
use serde::Deserialize;

use crate::error::{Error, Refusal, Result};
use crate::follow::{Followed, follow_lines};
use crate::paths::segment;
use crate::transport::Request;
use crate::{BoxClient, Follow};

#[derive(Clone, Debug, Deserialize)]
pub struct Outputs {
    #[serde(default, rename = "pendingComputers")]
    pub pending_computers: Vec<crate::areas::SetupComputer>,
    pub artifacts: Vec<Artifact>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Artifact {
    pub id: String,
    #[serde(rename = "artifactId")]
    pub artifact_id: Option<String>,
    pub path: String,
    pub name: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub gone: Option<f64>,
    pub latest: Version,
    pub computer: Option<OutputComputer>,
    #[serde(default)]
    pub away: bool,
    #[serde(rename = "lastSeen")]
    pub last_seen: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OutputComputer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub here: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Version {
    pub n: u32,
    pub hash: Option<String>,
    pub reason: Option<String>,
}

impl BoxClient {
    pub fn session_outputs(&self, session: &str, before: Option<&str>) -> Result<Outputs> {
        let mut route = format!("kuvo/setup/outputs?session={}", segment(session));
        if let Some(before) = before {
            route.push_str(&format!("&before={}", segment(before)));
        }
        self.setup_reader().get(&route)
    }

    pub fn follow_session_outputs(&self, session: &str) -> Follow<Followed<Outputs>> {
        let id = segment(session);
        follow_lines(&self.setup_reader(), format!("kuvo/setup/outputs/events?session={id}"))
    }

    pub fn artifact_bytes(&self, artifact: &Artifact) -> Result<Vec<u8>> {
        let id = segment(artifact.artifact_id.as_deref().unwrap_or(&artifact.id));
        let n = artifact.latest.n;
        let preview = if artifact.kind == "image" { "?preview=1" } else { "" };
        let setup = self.setup_reader();
        let owner = match &artifact.computer {
            Some(computer) if artifact.away => return Err(Error::Unreachable(computer.name.clone())),
            Some(computer) if !computer.here => setup.peer(&computer.id).ok_or_else(|| Error::Unreachable(computer.name.clone()))?,
            Some(_) => setup,
            None => self.clone(),
        };
        let response = owner.send(&Request::get(format!("kuvo/artifacts/{id}/versions/{n}{preview}")))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(Error::Refused(Refusal::from_answer(response.status, &response.body)))
        }
    }
}
