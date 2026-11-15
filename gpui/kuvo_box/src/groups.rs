//! A box's group chats (PROTOCOL.md, `kuvo/groups`): those that live on it, one chat's messages
//! followed as they come, who can be in one there (`kuvo/groups/joinable`), and what a client does
//! to them: write, stop, keep going, mark read, hand out a wrap-up's parts, make, change and
//! delete. The coding agents' bridge there runs them and decides every rule (who talks next, who a
//! message names, what a chat may be): this reads and sends.
//!
//! Times are as the box says them, seconds since 1970 (`time`, `next`, `started`, `finished`); the
//! app's clock is in milliseconds ([`GroupMessage::at_ms`]).

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::areas::Look;
use crate::client::BoxClient;
use crate::error::Result;
use crate::follow::{Follow, Followed, Timing, follow_lines};
use crate::paths;
use crate::transport::Request;

/// A bot as a chat lists it: who it is, how it looks (none: its description doesn't say), its own
/// conversation and folder there, and, for one of your other computers', that computer's name (its
/// conversation is there, not on the box asked).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GroupMember {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "lenient")]
    pub look: Option<Look>,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub directory: String,
    #[serde(default)]
    pub agents: bool,
    #[serde(default)]
    pub computer: Option<String>,
}

impl GroupMember {
    /// Its conversation, where it can be opened from the box asked: not one on another computer,
    /// nor one its computer didn't name.
    pub fn openable_session(&self) -> Option<&str> {
        (self.computer.is_none() && !self.session.is_empty()).then_some(self.session.as_str())
    }
}

/// Who does what after a wrap-up: its bot, its part, and how it's going (`proposed`, `waiting`,
/// `working`, `done`, `failed`, or a word a newer box adds), with the bots whose parts come first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GroupTask {
    pub id: String,
    pub bot: String,
    pub task: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub after: Vec<String>,
}

/// A message in a chat: who wrote it (none: you, or the chat itself), its words, when (seconds),
/// and what kind it is (`message`, `wrapUp`, `note`, `meeting`, or a newer box's word); a bot's
/// tool calls on its turn (as the agents' bridge records them), the parts a wrap-up hands out, and
/// the task a bot reports back on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GroupMessage {
    pub id: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub time: f64,
    #[serde(default = "message_kind")]
    pub kind: String,
    #[serde(default)]
    pub steps: Vec<Value>,
    #[serde(default, deserialize_with = "readable")]
    pub tasks: Vec<GroupTask>,
    #[serde(default)]
    pub report: Option<String>,
}

fn message_kind() -> String {
    "message".into()
}

impl GroupMessage {
    /// When it was said, in milliseconds since 1970 (the app's clock).
    pub fn at_ms(&self) -> f64 {
        self.time * 1000.0
    }
}

/// A chat's meeting: the next one (seconds), its topic, and how often it comes back (as kuvo-wake
/// writes it; none for once).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GroupMeeting {
    pub next: f64,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub every: Option<String>,
}

/// A chat that lives on the box asked, as `kuvo/groups` lists it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default, deserialize_with = "readable")]
    pub members: Vec<GroupMember>,
    /// The bot talking now.
    #[serde(default)]
    pub speaking: Option<String>,
    #[serde(default)]
    pub discussing: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub last: Option<GroupMessage>,
    #[serde(default, deserialize_with = "lenient")]
    pub meeting: Option<GroupMeeting>,
}

impl Group {
    /// One of its bots, by id.
    pub fn member(&self, id: &str) -> Option<&GroupMember> {
        self.members.iter().find(|member| member.id == id)
    }
}

/// What the bot talking is doing now: a tool it called (as a message's `steps` hold one, without
/// its output), the line its thinking is on, or its reply being written.
#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    Tool(Value),
    Thinking(String),
    Writing,
    /// A kind a newer box names.
    Other,
}

impl<'de> Deserialize<'de> for Activity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = Value::deserialize(deserializer)?;
        Ok(match raw.get("kind").and_then(Value::as_str) {
            Some("tool") => Activity::Tool(raw.get("call").cloned().unwrap_or(Value::Null)),
            Some("thinking") => Activity::Thinking(raw.get("text").and_then(Value::as_str).unwrap_or("").to_owned()),
            Some("writing") => Activity::Writing,
            _ => Activity::Other,
        })
    }
}

/// One chat as the box says it now: its latest messages (none: a line of its events whose
/// messages are as they were, so the ones shown stay), who's talking and what they're doing,
/// whether a discussion is under way or paused, and the bots whose part waits on their own
/// conversation, stopped.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Chat {
    #[serde(default)]
    pub messages: Option<Vec<GroupMessage>>,
    #[serde(default)]
    pub speaking: Option<String>,
    #[serde(default)]
    pub discussing: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, deserialize_with = "readable")]
    pub activity: Vec<Activity>,
    #[serde(default)]
    pub stopped: Vec<String>,
}

/// New Group, as it's sent: its name, its bots (by id, of those `kuvo/groups/joinable` names) and
/// what it's for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewGroup {
    pub name: String,
    pub members: Vec<String>,
    pub purpose: String,
}

/// A change to a chat: only what's given changes (an empty purpose is none).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroupChange {
    pub name: Option<String>,
    pub purpose: Option<String>,
    pub members: Option<Vec<String>>,
}

impl GroupChange {
    pub fn body(&self) -> Value {
        let mut body = Map::new();
        if let Some(name) = &self.name {
            body.insert("name".into(), json!(name));
        }
        if let Some(purpose) = &self.purpose {
            body.insert("purpose".into(), json!(purpose));
        }
        if let Some(members) = &self.members {
            body.insert("members".into(), json!(members));
        }
        Value::Object(body)
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.purpose.is_none() && self.members.is_none()
    }
}

/// One that doesn't read as a `T` (a newer box's shape) is none, not a failed answer.
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Option<T>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// Those of a list that read as a `T`: one a newer box words differently is left out, not the list.
fn readable<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Vec<T>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(value.as_array().map(|values| values.iter().filter_map(|value| serde_json::from_value(value.clone()).ok()).collect()).unwrap_or_default())
}

fn route(group: &str, rest: &str) -> String {
    format!("kuvo/groups/{}{rest}", paths::segment(group))
}

impl BoxClient {
    /// The group chats that live on this box.
    pub fn groups(&self) -> Result<Vec<Group>> {
        #[derive(Deserialize)]
        struct Answer {
            #[serde(deserialize_with = "readable")]
            groups: Vec<Group>,
        }
        Ok(self.get::<Answer>("kuvo/groups")?.groups)
    }

    /// One chat as it is now.
    pub fn group_chat(&self, group: &str) -> Result<Chat> {
        self.get(&route(group, ""))
    }

    /// Follows one chat: as it is now, then again as it changes (a line may leave its messages
    /// out: they're as they were).
    pub fn follow_group(&self, group: &str, timing: Timing) -> Follow<Followed<Chat>> {
        follow_lines(self, "kuvo-box group", route(group, "/events"), route(group, ""), timing)
    }

    /// Who can be in a chat made on this box: its bots, the first made first, then those of your
    /// other computers that take a chat's turns.
    pub fn joinable(&self) -> Result<Vec<GroupMember>> {
        #[derive(Deserialize)]
        struct Answer {
            #[serde(deserialize_with = "readable")]
            bots: Vec<GroupMember>,
        }
        Ok(self.get::<Answer>("kuvo/groups/joinable")?.bots)
    }

    /// Writes to a chat: only the words (the box there names whoever they name).
    pub fn write_to_group(&self, group: &str, text: &str) -> Result<()> {
        self.done(&Request::post(route(group, ""), json!({ "text": text })))
    }

    /// Stops the discussion under way.
    pub fn stop_group(&self, group: &str) -> Result<()> {
        self.done(&Request::post(route(group, "/stop"), json!({})))
    }

    /// Carries on a discussion paused after its turns.
    pub fn keep_going(&self, group: &str) -> Result<()> {
        self.done(&Request::post(route(group, "/keep-going"), json!({})))
    }

    /// Says you've read what's in it now.
    pub fn group_viewed(&self, group: &str) -> Result<()> {
        self.done(&Request::post(route(group, "/viewed"), json!({})))
    }

    /// Hands each proposed part of a wrap-up (`message`) to its bot.
    pub fn start_work(&self, group: &str, message: &str) -> Result<()> {
        self.done(&Request::post(route(group, "/start"), json!({ "message": message })))
    }

    /// Makes a group chat on this box: as the list has it now (none should it not list by then).
    pub fn make_group(&self, new: &NewGroup) -> Result<Option<Group>> {
        let mut body = json!({ "name": new.name, "members": new.members });
        if !new.purpose.trim().is_empty() {
            body["purpose"] = json!(new.purpose);
        }
        self.changed(Request::post("kuvo/groups", body))
    }

    /// Changes a chat: as the list has it now (none should it not list by then).
    pub fn change_group(&self, group: &str, change: &GroupChange) -> Result<Option<Group>> {
        self.changed(Request { method: "PATCH".into(), path: route(group, ""), headers: Vec::new(), body: Some(change.body()) })
    }

    /// Deletes a chat: its messages and seats go, and its bots forget it.
    pub fn delete_group(&self, group: &str) -> Result<()> {
        self.done(&Request::delete(route(group, "")))
    }

    fn changed(&self, request: Request) -> Result<Option<Group>> {
        #[derive(Deserialize)]
        struct Answer {
            #[serde(default, deserialize_with = "lenient")]
            group: Option<Group>,
        }
        Ok(self.value::<Answer>(&request)?.group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat's events may leave its messages out: they're as they were, not none.
    #[test]
    fn a_line_without_messages_isnt_an_empty_chat() {
        let line: Chat = serde_json::from_value(json!({ "speaking": "bot_mira", "discussing": true, "paused": false, "activity": [{ "kind": "thinking", "text": "Weighing it" }, { "kind": "later" }] })).unwrap();
        assert_eq!(line.messages, None);
        assert_eq!(line.activity, vec![Activity::Thinking("Weighing it".into()), Activity::Other]);
        let full: Chat = serde_json::from_value(json!({ "messages": [], "discussing": false, "paused": false })).unwrap();
        assert_eq!(full.messages, Some(Vec::new()));
    }

    /// Times are the box's seconds; a member of another computer isn't opened from here.
    #[test]
    fn a_message_is_read_in_the_boxs_seconds_and_a_member_elsewhere_isnt_opened() {
        let group: Group = serde_json::from_value(json!({
            "id": "grp_1", "name": "Crew", "discussing": false, "paused": false, "unread": false,
            "members": [
                { "id": "bot_a", "name": "Ada", "session": "ses_a", "directory": "/a", "agents": false },
                { "id": "bot_b", "name": "Bo", "session": "ses_b", "directory": "", "agents": false, "computer": "devbox", "look": { "color": "sky", "shape": "round", "face": "calm" } },
                { "id": "bot_c", "name": "Cy", "look": 7, "session": "", "directory": "", "agents": false }
            ],
            "last": { "id": "m1", "text": "Done.", "time": 1_794_650_000.0, "kind": "message" }
        }))
        .unwrap();
        assert_eq!(group.last.unwrap().at_ms(), 1_794_650_000_000.0);
        assert_eq!(group.members[0].openable_session(), Some("ses_a"));
        assert_eq!(group.members[1].openable_session(), None, "its conversation is on devbox");
        assert_eq!(group.members[2].look, None, "a look it can't read is none, not the member");
    }
}
