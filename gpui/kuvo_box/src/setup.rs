//! Your setup as the box says it (PROTOCOL.md, "Your setup"): first run, account readiness and
//! merging two setups.

use serde::Deserialize;
use serde_json::json;

use crate::areas::ComputerInstallerCode;
use crate::client::decode;
use crate::client::BoxClient;
use crate::error::Result;
use crate::transport::Request;

#[derive(Deserialize)]
struct Welcomed {
    welcomed: bool,
}

/// An action available before any local box can answer setup requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupOffer {
    AddComputer,
    ReinstallLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SetupMergeTarget {
    pub setup: String,
    pub primary: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SetupMergeReceipt {
    pub operation: String,
    pub setup: String,
    pub primary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SetupMergeStatus {
    pub can: bool,
    pub why: Option<String>,
    pub pending: Option<SetupMergeTarget>,
    pub completed: Option<SetupMergeReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SetupLeadership {
    pub primary: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupLeadershipReply {
    Ok(SetupLeadership),
    Accepted(SetupLeadership),
}

/// Whether a first run has anything to ask about your accounts on a box, as it says
/// (`kuvo/setup/ready`): the box decides it, a client only goes by it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Ready {
    /// Nothing to ask: a model on an account of yours runs and no plan needs you. A first run
    /// passes over its accounts step.
    pub ready: bool,
    /// A model that needs an account runs here now.
    #[serde(default)]
    pub runs: bool,
    /// The plans whose sign-in ended or says something's wrong, by id.
    #[serde(default, rename = "needsYou")]
    pub needs_you: Vec<String>,
    /// The sides that can't say yet (`engine`, `agents`): what's said may change in a moment,
    /// and a client asks again.
    #[serde(default)]
    pub pending: Vec<String>,
}

/// Who's in your setup and who leads it, as this box says it (`kuvo/setup`, PROTOCOL.md "Your
/// setup"): facts only, the same on every box of one setup. In no setup, nulls and no computers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct SetupView {
    #[serde(default)]
    pub setup: Option<String>,
    #[serde(default)]
    pub epoch: Option<u64>,
    #[serde(default)]
    pub primary: Option<SetupMember>,
    /// The computer that takes over if the main computer is lost, by its key.
    #[serde(default)]
    pub standby: Option<String>,
    #[serde(default, rename = "leadershipWarning")]
    pub leadership_warning: Option<String>,
    #[serde(default)]
    pub computers: Vec<SetupMember>,
}

/// One computer in your setup: its key, name, kind and role (`primary` or `member`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct SetupMember {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub role: Option<String>,
}

impl SetupView {
    /// Whether this box is in a setup with a main computer.
    pub fn is_set_up(&self) -> bool {
        self.setup.is_some() && self.primary.is_some()
    }

    pub fn primary_key(&self) -> Option<&str> {
        self.primary.as_ref().map(|primary| primary.key.as_str())
    }

    pub fn member(&self, key: &str) -> Option<&SetupMember> {
        self.computers.iter().find(|member| member.key == key)
    }
}

impl BoxClient {
    /// Your setup as this box says it (`kuvo/setup`).
    pub fn setup_view(&self) -> Result<SetupView> {
        self.get("kuvo/setup")
    }

    /// Selects a standby at the setup position the client observed.
    pub fn select_setup_standby(&self, standby: &str, epoch: u64) -> Result<SetupLeadershipReply> {
        self.setup_leadership_change("kuvo/setup/standby", json!({ "standby": standby, "epoch": epoch }))
    }

    /// Hands leadership to a member at the setup position the client observed.
    pub fn hand_over_setup(&self, primary: &str, epoch: u64) -> Result<SetupLeadershipReply> {
        self.setup_leadership_change("kuvo/setup/handover", json!({ "primary": primary, "epoch": epoch }))
    }

    fn setup_leadership_change(&self, route: &str, body: serde_json::Value) -> Result<SetupLeadershipReply> {
        let response = self.send(&Request::post(route, body))?;
        let position = decode(&response)?;
        Ok(if response.status == 202 { SetupLeadershipReply::Accepted(position) } else { SetupLeadershipReply::Ok(position) })
    }

    /// Ask the setup staying in charge for a code bound to the joining primary.
    pub fn setup_merge_code(&self, source: &str, computer: &str) -> Result<ComputerInstallerCode> {
        self.value(&Request::post("kuvo/computers/add/merge-code", json!({ "source": source, "computer": computer })))
    }

    /// Reissue a code for the same interrupted operation, including after a primary change.
    pub fn resume_setup_merge_code(&self, source: &str, computer: &str, operation: &str) -> Result<ComputerInstallerCode> {
        self.value(&Request::post("kuvo/computers/add/merge-code", json!({ "source": source, "computer": computer, "resume": operation })))
    }

    /// Read merge eligibility or the saved result on the joining computer.
    pub fn setup_merge_status(&self) -> Result<SetupMergeStatus> {
        self.get("kuvo/setup/merge")
    }

    /// Bring over the setup on this computer. Retrying an interrupted save uses the same code.
    pub fn join_setup_merge(&self, code: &str) -> Result<SetupMergeReceipt> {
        self.value(&Request::post("kuvo/setup/merge", json!({ "code": code })))
    }

    /// Whether a first run has anything to ask about your accounts on this box, as it says now.
    pub fn first_run_ready(&self) -> Result<Ready> {
        self.get("kuvo/setup/ready")
    }

    /// Whether you've had the first run with this box.
    pub fn welcomed(&self) -> Result<bool> {
        self.get::<Welcomed>("kuvo/setup").map(|setup| setup.welcomed)
    }

    /// Says the first run with this box is done (`true`), or to be had again (`false`); answers
    /// how the box has it now.
    pub fn set_welcomed(&self, welcomed: bool) -> Result<bool> {
        self.value::<Welcomed>(&Request::post("kuvo/setup/welcomed", json!({ "welcomed": welcomed }))).map(|said| said.welcomed)
    }
}
