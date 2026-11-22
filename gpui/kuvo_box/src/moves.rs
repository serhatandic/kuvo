//! Move to Computer routes. The source computer makes the decision and reports the result.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    BoxClient, Result,
    accounts::Confirm,
    follow::{Follow, Followed, follow_lines},
    paths::query_value,
    transport::Request,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bot,
    Chat,
    Group,
}

impl Kind {
    fn query(self) -> &'static str {
        match self {
            Self::Bot => "bot",
            Self::Chat => "chat",
            Self::Group => "group",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Target {
    pub kind: Kind,
    pub id: String,
}

impl Target {
    pub fn new(kind: Kind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Places {
    pub places: Vec<Place>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Place {
    pub key: String,
    pub name: String,
    pub ok: bool,
    pub why: Option<String>,
    pub confirm: Option<Confirm>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Moving {
    pub id: String,
    pub kind: Kind,
    pub thing: String,
    pub to: String,
    pub stage: String,
    pub place: Option<String>,
    pub progress: Option<String>,
    pub sent: Option<u64>,
    pub size: Option<u64>,
    #[serde(default)]
    pub waiting: bool,
}

impl Moving {
    pub fn fraction(&self) -> Option<f32> {
        let (sent, size) = (self.sent?, self.size?);
        (self.stage == "copying" && !self.waiting && size > 0)
            .then(|| sent.min(size) as f32 / size as f32)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Moves {
    pub moves: Vec<Moving>,
    #[serde(default)]
    pub elsewhere: Vec<Elsewhere>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Elsewhere {
    pub moves: Vec<Moving>,
}

impl Moves {
    pub fn of(&self, target: &Target) -> Option<&Moving> {
        self.moves
            .iter()
            .chain(self.elsewhere.iter().flat_map(|peer| &peer.moves))
            .find(|moving| moving.kind == target.kind && moving.thing == target.id)
    }
}

#[derive(Debug, Deserialize)]
pub struct Accepted {
    #[serde(rename = "move")]
    pub moving: Moving,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Receipt {
    Moving {
        #[serde(rename = "move")]
        moving: Moving,
    },
    Done {
        #[serde(rename = "move")]
        moved: Moving,
        message: String,
        note: Option<String>,
    },
    Failed {
        message: String,
    },
}

impl BoxClient {
    pub fn move_places(&self, target: &Target) -> Result<Places> {
        self.get(&format!(
            "kuvo/moves/places?kind={}&id={}",
            target.kind.query(),
            query_value(&target.id)
        ))
    }

    pub fn begin_move(&self, target: &Target, to: &str) -> Result<Accepted> {
        self.value(&Request::post(
            "kuvo/moves",
            json!({
                "kind": target.kind, "id": target.id, "to": to, "receipt": true,
            }),
        ))
    }

    pub fn move_receipt(&self, id: &str) -> Result<Receipt> {
        self.get(&format!("kuvo/moves/receipt?id={}", query_value(id)))
    }

    pub fn moves(&self) -> Result<Moves> {
        self.get("kuvo/moves")
    }

    pub fn follow_moves(&self) -> Follow<Followed<Moves>> {
        follow_lines(self, "kuvo/moves/events".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_places_read_confirmation_and_refusals_as_sent() {
        let places: Places = serde_json::from_value(json!({"places":[
            {"key":"lab","name":"Lab","ok":true,"confirm":{"title":"Move Mira to Lab?","message":"A turn is running.","label":"Move"}},
            {"key":"studio","name":"Studio","ok":false,"why":"Studio isn’t connected. Connect to it, then try again.","note":"Can’t be reached"}
        ]})).unwrap();
        assert_eq!(
            places.places[0].confirm.as_ref().unwrap().message,
            "A turn is running."
        );
        assert!(!places.places[1].ok);
        assert_eq!(
            places.places[1].why.as_deref(),
            Some("Studio isn’t connected. Connect to it, then try again.")
        );
    }

    #[test]
    fn move_states_read_progress_waiting_and_terminal_results() {
        let mut moving = json!({"id":"mov_one","kind":"bot","thing":"bot_mira","to":"lab","place":"Lab",
            "stage":"copying","sent":25,"size":100,"progress":"Moving to Lab…"});
        let state: Moves =
            serde_json::from_value(json!({"moves":[], "elsewhere":[{"moves":[moving.clone()]}]}))
                .unwrap();
        let shown = state.of(&Target::new(Kind::Bot, "bot_mira")).unwrap();
        assert_eq!(shown.fraction(), Some(0.25));
        assert_eq!(shown.progress.as_deref(), Some("Moving to Lab…"));
        assert!(state.of(&Target::new(Kind::Chat, "bot_mira")).is_none());
        moving["waiting"] = true.into();
        moving["progress"] = "Waiting for Lab…".into();
        let read = |value| serde_json::from_value::<Receipt>(value).unwrap();
        assert!(
            matches!(read(json!({"state":"moving","move":moving.clone()})),
            Receipt::Moving { moving } if moving.fraction().is_none() && moving.progress.as_deref() == Some("Waiting for Lab…"))
        );
        assert!(
            matches!(read(json!({"state":"done","move":moving,"message":"Moved to Lab.","note":"Uses the account on Lab."})),
            Receipt::Done { moved, message, note: Some(note) } if moved.to == "lab" && message == "Moved to Lab." && note == "Uses the account on Lab.")
        );
        assert!(
            matches!(read(json!({"state":"failed","message":"Lab couldn’t take it."})),
            Receipt::Failed { message } if message == "Lab couldn’t take it.")
        );
    }
}
