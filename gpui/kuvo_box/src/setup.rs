//! Your setup as the box says it (PROTOCOL.md, "Your setup"): first run, account readiness and
//! merging two setups.

use serde::Deserialize;
use serde_json::json;

use crate::areas::ComputerInstallerCode;
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

impl BoxClient {
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
