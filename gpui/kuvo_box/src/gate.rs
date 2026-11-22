//! The update gate (PROTOCOL.md, "The update gate", #679), as the box says it: every box states how
//! it stands with this client (`compatibility` in its `kuvo/info`), and this client draws that and
//! decides nothing of its own. A box that's *behind* is used as it is, each feature drawn where it
//! has it (its `missing` names what it lacks); one that's *held* can't be used here until it's
//! updated, and shows its update in place of its areas. Every request still goes to the box, which
//! answers it or refuses it in its own words. [`REQUIRED`] names what this client draws, the same
//! list as `protocol/gate-required.txt`, which the box reads to say its `compatibility`.

mod required;
pub use required::REQUIRED;

use crate::info::Compatibility;

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
            Compatibility::Behind { missing } => Gate::Behind { missing: missing.clone() },
            Compatibility::Held { missing } => Gate::Held { missing: missing.clone() },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_is_what_the_box_says() {
        let behind: Compatibility = serde_json::from_value(serde_json::json!({ "state": "behind", "missing": ["bots.state"] })).unwrap();
        let gate = Gate::said(&behind);
        assert!(!gate.is_held() && gate.is_older() && !gate.has("bots.state") && gate.has("sessions.list"));
        let held: Compatibility = serde_json::from_value(serde_json::json!({ "state": "held", "missing": ["messages.send"] })).unwrap();
        assert!(Gate::said(&held).is_held());
        // (One that didn't say is used as it is: never held by this client's own reading.)
        assert_eq!(Gate::said(&Compatibility::Unknown), Gate::Open);
    }
}
