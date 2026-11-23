//! A session's menu, done by its box (PROTOCOL.md, "A session's actions", `sessions.actions`): what
//! it offers now, each with its button's words and the confirm it asks first, all the box's, and
//! Pin, Rename, Archive and Delete themselves. The box writes the notes the lists go by and refuses
//! one moving to another computer in words; a client draws the menu and the box's answer.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Confirm;
use crate::client::BoxClient;
use crate::error::Result;
use crate::paths::segment;
use crate::transport::Request;

/// Something its menu offers now: which, its button's words, whether it destroys something, and
/// the confirm a client asks first, where it asks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Action {
    /// `pin`, `unpin`, `rename`, `archive`, `unarchive`, `delete`; a client draws the ones it has a
    /// control for.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub destructive: bool,
    #[serde(default)]
    pub confirm: Option<Confirm>,
}

/// What a session's menu offers now, in order; `held`: why none of it can be done now (it's moving
/// to another computer), in words to show.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Actions {
    pub actions: Vec<Action>,
    #[serde(default)]
    pub held: Option<String>,
}

/// The session as its box has it after a change (as the engine or the coding agents' bridge lists
/// it).
#[derive(Debug, Clone, Deserialize)]
struct Changed {
    info: Value,
}

fn route(session: &str, action: &str) -> String {
    format!("kuvo/sessions/{}/{action}", segment(session))
}

impl BoxClient {
    /// What a session's menu offers now.
    pub fn session_actions(&self, session: &str) -> Result<Actions> {
        self.get(&route(session, "actions"))
    }

    /// Pins it to the top of the lists, or takes it off: the session as it is now.
    pub fn pin_session(&self, session: &str, pinned: bool) -> Result<Value> {
        self.change(session, "pin", json!({ "pinned": pinned }))
    }

    /// Archives it, or brings it back.
    pub fn archive_session(&self, session: &str, archived: bool) -> Result<Value> {
        self.change(session, "archive", json!({ "archived": archived }))
    }

    /// Keeps `directory` (an absolute folder on its computer) as one of its box's projects, or
    /// brings back one removed; one kept already stays as it is (`PUT kuvo/projects`).
    pub fn add_project(&self, directory: &str) -> Result<()> {
        let path = format!("kuvo/projects?directory={}", crate::paths::query_value(directory));
        let _: Value = self.value(&Request::put(path, json!({})))?;
        Ok(())
    }

    /// Names it as written (the box trims it, and refuses none).
    pub fn rename_session(&self, session: &str, title: &str) -> Result<Value> {
        self.change(session, "rename", json!({ "title": title }))
    }

    /// Deletes it for good, where it runs (one gone already is done too).
    pub fn delete_session(&self, session: &str) -> Result<()> {
        let _: Value = self.value(&Request::delete(format!("kuvo/sessions/{}", segment(session))))?;
        Ok(())
    }

    /// What deleting several at once comes to (Delete All), in the box's words: asked once, then
    /// each is deleted with `delete_session`.
    pub fn deletes_confirm(&self, sessions: &[&str]) -> Result<Confirm> {
        self.value(&Request::post("kuvo/sessions/deletes", json!({ "sessions": sessions })))
    }

    fn change(&self, session: &str, action: &str, body: Value) -> Result<Value> {
        let changed: Changed = self.value(&Request::post(route(session, action), body))?;
        Ok(changed.info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PROTOCOL.md, "A session's actions": the menu as the box says it, Delete's confirm in its
    /// words, and one it doesn't know read all the same (a client leaves it out).
    #[test]
    fn the_menu_reads_as_the_box_says_it() {
        let actions: Actions = serde_json::from_value(json!({
            "session": "ses_a",
            "actions": [
                { "id": "pin", "label": "Pin" },
                { "id": "share", "label": "Share" },
                { "id": "delete", "label": "Delete", "destructive": true, "confirm": { "title": "Delete “Trip”?", "message": "Its worktree is deleted too.", "label": "Delete" } }
            ],
            "held": "It’s moving to another computer. Try again once it’s there."
        }))
        .unwrap();
        assert_eq!(actions.actions.iter().map(|action| (action.id.as_str(), action.destructive)).collect::<Vec<_>>(), vec![("pin", false), ("share", false), ("delete", true)]);
        assert_eq!(actions.actions[2].confirm.as_ref().map(|confirm| confirm.message.as_str()), Some("Its worktree is deleted too."));
        assert!(actions.held.is_some());
    }
}
