//! How much a conversation's agent does on its own (PROTOCOL.md, "Approval"): kept on the box with
//! the conversation, which says what it comes to here now and what each can be set to, so the
//! composer's mode menu draws what the box says. Full Access is confirmed once per computer
//! (`PATCH kuvo/approval`), the first time it's picked there.

use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Refusal, Result};
use crate::paths;
use crate::transport::Request;
use crate::BoxClient;

/// One of the modes, in the menu's order. One a newer box names is `Other`, shown by its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(from = "String")]
pub enum Approval {
    AskFirst,
    AcceptEdits,
    Auto,
    FullAccess,
    Other(String),
}

impl From<String> for Approval {
    fn from(name: String) -> Approval {
        match name.as_str() {
            "askFirst" => Approval::AskFirst,
            "acceptEdits" => Approval::AcceptEdits,
            "auto" => Approval::Auto,
            "fullAccess" => Approval::FullAccess,
            _ => Approval::Other(name),
        }
    }
}

impl Approval {
    /// The four the menu offers, in its order.
    pub const ALL: [Approval; 4] = [Approval::AskFirst, Approval::AcceptEdits, Approval::Auto, Approval::FullAccess];

    /// Its name on the box.
    pub fn name(&self) -> &str {
        match self {
            Approval::AskFirst => "askFirst",
            Approval::AcceptEdits => "acceptEdits",
            Approval::Auto => "auto",
            Approval::FullAccess => "fullAccess",
            Approval::Other(name) => name,
        }
    }

    /// Its name in the menu, as every client words it (KuvoClient's `ApprovalWords`).
    pub fn title(&self) -> &str {
        match self {
            Approval::AskFirst => "Ask First",
            Approval::AcceptEdits => "Accept Edits",
            Approval::Auto => "Auto",
            Approval::FullAccess => "Full Access",
            Approval::Other(name) => name,
        }
    }

    /// What it does, in a few words.
    pub fn detail(&self) -> &str {
        match self {
            Approval::AskFirst => "Asks before it acts",
            Approval::AcceptEdits => "Asks before commands",
            Approval::Auto => "Sandbox, then review",
            Approval::FullAccess => "No sandbox, no asks",
            Approval::Other(_) => "",
        }
    }
}

/// Why a mode does less than picked for now, or can't be picked: `sandbox` (Auto without the whole
/// sandbox), `fullAccessOff` (not confirmed on this computer yet), and the box's words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Why {
    pub reason: String,
    pub why: String,
}

/// One of the modes as the box offers it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Choice {
    pub approval: Approval,
    #[serde(default)]
    pub held: Option<Why>,
    #[serde(default)]
    pub unavailable: Option<Why>,
}

/// A conversation's mode as its box shows it (`GET kuvo/sessions/<id>/approval`): the one picked,
/// what it comes to here now and why that's less, and each it can be set to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Shown {
    pub approval: Approval,
    pub effective: Approval,
    #[serde(default)]
    pub held: Option<Why>,
    #[serde(default)]
    pub options: Vec<Choice>,
}

impl Shown {
    /// Why it does less than picked for now (Auto asking first without the sandbox).
    pub fn holding(&self) -> Option<&Why> {
        (self.effective != self.approval).then_some(self.held.as_ref()).flatten()
    }

    pub fn choice(&self, approval: &Approval) -> Option<&Choice> {
        self.options.iter().find(|choice| &choice.approval == approval)
    }
}

/// Whether Auto runs, for the engine's conversations and the coding agents'.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AutoHere {
    #[serde(default)]
    pub held: Option<Why>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Autos {
    #[serde(default)]
    pub engine: AutoHere,
    #[serde(default)]
    pub agents: AutoHere,
}

/// Each mode a new conversation can start in, on the engine and on the coding agents, as the box
/// says them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct HereOptions {
    #[serde(default)]
    pub engine: Vec<Choice>,
    #[serde(default)]
    pub agents: Vec<Choice>,
}

/// A computer's modes (`GET kuvo/approval`): whether you confirmed Full Access there, the one a new
/// conversation starts with, why Auto asks first there for now, and each a new one can start in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Here {
    pub full_access: bool,
    pub default: Approval,
    #[serde(default)]
    pub auto: Autos,
    #[serde(default)]
    pub options: HereOptions,
}

impl Here {
    /// As a new conversation's composer shows `approval` (none to read yet): each mode as the box
    /// offers it here; one it holds asks first for now.
    pub fn shown(&self, approval: Approval) -> Shown {
        let options = self.options.engine.clone();
        let held = options.iter().find(|choice| choice.approval == approval).and_then(|choice| choice.held.clone());
        Shown { effective: if held.is_some() { Approval::AskFirst } else { approval.clone() }, held, options, approval }
    }
}

/// What a pick came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// Set: the conversation's mode as the box shows it now.
    Set(Shown),
    /// Full Access isn't confirmed on this computer: ask, then confirm and pick it again.
    ConfirmFirst,
}

fn route(session: &str) -> String {
    format!("kuvo/sessions/{}/approval", paths::segment(session))
}

impl BoxClient {
    /// A conversation's mode as its box shows it.
    pub fn session_approval(&self, session: &str) -> Result<Shown> {
        self.get(&route(session))
    }

    /// Sets a conversation's mode; Full Access where it isn't confirmed here asks first.
    pub fn set_session_approval(&self, session: &str, approval: &Approval) -> Result<Picked> {
        let request = Request { method: "PUT".into(), path: route(session), headers: Vec::new(), body: Some(json!({ "approval": approval.name() })) };
        let response = self.send(&request)?;
        if response.status == 409 && reason(&response.body).as_deref() == Some("fullAccessOff") {
            return Ok(Picked::ConfirmFirst);
        }
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        serde_json::from_slice(&response.body).map(Picked::Set).map_err(|error| Error::BadAnswer(error.to_string()))
    }

    /// This computer's modes.
    pub fn approval_here(&self) -> Result<Here> {
        self.get("kuvo/approval")
    }

    /// You confirmed Full Access on this computer: it may be picked here from now on.
    pub fn allow_full_access(&self) -> Result<()> {
        let request = Request { method: "PATCH".into(), path: "kuvo/approval".into(), headers: Vec::new(), body: Some(json!({ "fullAccess": true })) };
        let response = self.send(&request)?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        Ok(())
    }
}

fn reason(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body).ok()?.get("reason")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_conversation_shows_each_mode_as_the_box_offers_it_here() {
        let here: Here = serde_json::from_str(r#"{"fullAccess":false,"default":"auto","auto":{"engine":{},"agents":{}},"options":{"engine":[{"approval":"askFirst"},{"approval":"acceptEdits"},{"approval":"auto","held":{"reason":"sandbox","why":"The sandbox isn't running."}},{"approval":"fullAccess","unavailable":{"reason":"fullAccessOff","why":"Confirm it once on this computer first."}}],"agents":[]}}"#).unwrap();
        let shown = here.shown(Approval::Auto);
        assert_eq!(shown.effective, Approval::AskFirst, "Auto asks first while the box holds it");
        assert_eq!(shown.choice(&Approval::FullAccess).and_then(|choice| choice.unavailable.as_ref()).map(|why| why.reason.as_str()), Some("fullAccessOff"));
        assert_eq!(here.shown(Approval::AskFirst).effective, Approval::AskFirst);
    }

    #[test]
    fn a_mode_a_newer_box_names_is_kept_by_its_name() {
        let shown: Shown = serde_json::from_str(r#"{"approval":"careful","effective":"askFirst","options":[]}"#).unwrap();
        assert_eq!(shown.approval, Approval::Other("careful".into()));
        assert_eq!(shown.approval.title(), "careful");
    }
}
