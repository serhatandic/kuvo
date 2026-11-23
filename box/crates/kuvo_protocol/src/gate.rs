//! Update compatibility as reported by the computer. Clients use its decision.

use serde::{Deserialize, Serialize};

pub use crate::REQUIRED;

use crate::BASELINE;

/// Judge the features served by this computer. Missing baseline capabilities hold the gate.
pub fn compatibility(features: &[&str]) -> Compatibility {
    let missing: Vec<_> = REQUIRED
        .iter()
        .filter(|name| !features.contains(name))
        .map(|name| (*name).to_owned())
        .collect();
    if missing.is_empty() {
        Compatibility::Open
    } else if BASELINE.iter().any(|name| !features.contains(name)) {
        Compatibility::Held { missing }
    } else {
        Compatibility::Behind { missing }
    }
}

/// How the box says it stands with this client (`compatibility`): every client draws it as it is,
/// the box's word alone. `missing`: what this client draws that it lacks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Compatibility {
    Open,
    /// Used as it is, what it lacks drawn without.
    Behind {
        #[serde(default)]
        missing: Vec<String>,
    },
    /// This client can't use it until it's updated.
    Held {
        #[serde(default)]
        missing: Vec<String>,
    },
    /// It didn't say (a box from before it did).
    #[default]
    #[serde(other)]
    Unknown,
}

impl Compatibility {
    pub fn is_held(&self) -> bool {
        matches!(self, Self::Held { .. })
    }

    pub fn is_older(&self) -> bool {
        matches!(self, Self::Behind { .. } | Self::Held { .. })
    }

    pub fn is_behind(&self) -> bool {
        matches!(self, Self::Behind { .. })
    }
}

/// Where a box stands with this client, as it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Not read yet: the first request reads `kuvo/info` before it goes.
    Unknown,
    /// It has everything this client draws (or didn't say otherwise).
    Open,
    /// It's older than this client, lacking these features: used as it is (what it lacks isn't
    /// drawn), and offered its update.
    Behind { missing: Vec<String> },
    /// It says this client can't use it (`missing`: what it lacks), or answers no `kuvo/info` at
    /// all: it waits for its update.
    Held { missing: Vec<String> },
}

impl Gate {
    /// The gate as the box's `kuvo/info` says it (`compatibility`).
    pub fn said(compatibility: &Compatibility) -> Gate {
        match compatibility {
            Compatibility::Open | Compatibility::Unknown => Gate::Open,
            Compatibility::Behind { missing } => Gate::Behind {
                missing: missing.clone(),
            },
            Compatibility::Held { missing } => Gate::Held {
                missing: missing.clone(),
            },
        }
    }

    /// Whether the box has `feature`, as it says: drawn where it has, drawn without where it
    /// hasn't; nothing while it's unknown or held.
    pub fn has(&self, feature: &str) -> bool {
        match self {
            Gate::Open => true,
            Gate::Behind { missing } => !missing.iter().any(|name| name == feature),
            Gate::Unknown | Gate::Held { .. } => false,
        }
    }

    /// It says this client can't use it: only its update is shown.
    pub fn is_held(&self) -> bool {
        matches!(self, Gate::Held { .. })
    }

    /// It's older than this client (held, or only behind): its update is offered.
    pub fn is_older(&self) -> bool {
        matches!(self, Gate::Held { .. } | Gate::Behind { .. })
    }
}
