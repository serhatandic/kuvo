//! Your setup as a box says it (PROTOCOL.md, "Your setup", `kuvo/setup`): here only whether you've
//! had Kuvo Code's first run with this box (`welcomed`, `setup.welcomed`), kept on the box so
//! every client of it agrees, and saying it's done (or to be had again); and whether a first run
//! has anything to ask about your accounts there (`kuvo/setup/ready`, `setup.ready`), which the box
//! decides.

use serde::Deserialize;
use serde_json::json;

use crate::client::BoxClient;
use crate::error::Result;
use crate::transport::Request;

#[derive(Deserialize)]
struct Welcomed {
    welcomed: bool,
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
}

impl BoxClient {
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
