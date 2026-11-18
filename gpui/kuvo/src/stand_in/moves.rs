//! Move receipts and places for desktop tests and shots.

use kuvo_box::transport::Request;
use serde_json::{Value, json};

use super::{StandIn, State};

#[derive(Clone, Copy)]
pub enum Scenario {
    Ready,
    Copying,
    Failed,
    ReceiptRevoked,
}

impl StandIn {
    pub fn set_move_scenario(&self, scenario: Scenario) {
        let mut state = self.state();
        state.move_scenario = Some(scenario);
        if let Some(mira) = state
            .bots
            .bots
            .iter_mut()
            .find(|bot| bot["id"] == "bot_mira")
        {
            mira.as_object_mut()
                .expect("the bot description")
                .remove("project");
        }
        state.sync_bots();
    }
}

fn moving(phase: &str, progress: &str) -> Value {
    json!({
        "id": "mov_mira", "kind": "bot", "thing": "bot_mira", "name": "Mira",
        "from": "mac", "to": "key_studio", "place": "Studio", "role": "from",
        "stage": phase, "sessions": ["ses_mira"], "progress": progress,
    })
}

pub(super) fn answer(state: &mut State, request: &Request) -> Option<(u16, Value)> {
    use Scenario::*;
    let scenario = state.move_scenario;
    Some(match (request.method.as_str(), request.route(), scenario) {
        ("GET", "kuvo/moves", Some(Copying)) => {
            let mut copy = moving("copying", "Moving to Studio…");
            copy["sent"] = json!(14_000_000);
            copy["size"] = json!(32_000_000);
            (200, json!({ "moves": [copy], "away": [] }))
        }
        ("GET", "kuvo/moves", _) => (200, json!({ "moves": [], "away": [] })),
        ("GET", "kuvo/moves/places", Some(_)) => (
            200,
            json!({ "places": [
                { "key": "key_studio", "name": "Studio", "kind": "machine", "ok": true, "confirm": {
                    "title": "Move “Mira” to Studio?",
                    "message": "Its memory, files, wake-ups and conversation move with it. A turn is running. The move waits up to a minute for it to finish. New messages and wake-ups wait during the move. Vault items, integrations and browser sign-ins stay on Studio Mac.",
                    "label": "Move",
                }},
                { "key": "key_macbook", "name": "MacBook Air", "kind": "mac", "ok": false,
                  "why": "MacBook Air isn’t connected. Connect to it, then try again.", "note": "Can’t be reached" },
            ]}),
        ),
        ("POST", "kuvo/moves", Some(_)) => (
            202,
            json!({ "move": moving("holding", "Getting ready to move to Studio…") }),
        ),
        ("GET", "kuvo/moves/receipt", Some(ReceiptRevoked)) => {
            (403, json!({"message": "Access was revoked."}))
        }
        ("GET", "kuvo/moves/receipt", Some(Failed)) => (
            200,
            json!({
                "state": "failed", "message": "The engine on Studio isn’t running. Try again once it is.",
            }),
        ),
        ("GET", "kuvo/moves/receipt", Some(Ready)) => {
            state.bots.move_away("bot_mira", "Studio");
            state.sync_bots();
            (
                200,
                json!({ "state": "done", "move": moving("finishing", "Finishing the move to Studio…"), "message": "Moved to Studio." }),
            )
        }
        _ => return None,
    })
}
