//! The shared update gate as reported by the computer.

pub use kuvo_protocol::gate::{Gate, REQUIRED};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::info::Compatibility;

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
