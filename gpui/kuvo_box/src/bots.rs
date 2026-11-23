//! A box's bots, as it lists, makes, changes and deletes them (PROTOCOL.md, `kuvo/bots`): the list
//! with what's held for another computer and what one refused, what New Bot offers there
//! (`kuvo/bots/templates`), `POST kuvo/bots`, `PATCH` and `DELETE kuvo/bots/<id>`, and a bot's
//! sessions (`kuvo/work?bot=`). The box decides all of it: what a bot may be called, which looks,
//! personalities and models there are, and where an edit waits. This reads and sends.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::areas::{Bot, DeleteLine, Look, readable};
use crate::client::{BoxClient, decode};
use crate::error::{Error, Refusal, Result};
use crate::transport::Request;
use crate::typed::ModelRef;

/// A change or a delete a box holds for the computer a bot lives on (`bots.held`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Held {
    /// The bot's id.
    pub bot: String,
    /// The name of the computer it waits for, as the box that holds it knows it.
    #[serde(default)]
    pub computer: String,
    /// The bot's name and look, as its computer last told the box (`bots.elsewhere`); none for a
    /// bot no computer of yours ever listed to it.
    #[serde(default, rename = "name")]
    pub bot_name: Option<String>,
    #[serde(default, rename = "look", deserialize_with = "lenient")]
    pub bot_look: Option<Look>,
    /// Each field as `PATCH` takes it, the last given of each.
    #[serde(default)]
    pub change: Option<Map<String, Value>>,
    /// It's a delete.
    #[serde(default)]
    pub delete: bool,
    /// When it was first held (ms).
    #[serde(default)]
    pub since: Option<f64>,
    /// Why it hasn't gone to a computer that answers, in the box's words.
    #[serde(default)]
    pub waits: Option<String>,
}

impl Held {
    /// The name it was given in what's held, if that changes it.
    pub fn name(&self) -> Option<&str> {
        self.change.as_ref()?.get("name")?.as_str()
    }

    /// The look it was given in what's held, if that changes it.
    pub fn look(&self) -> Option<Look> {
        serde_json::from_value(self.change.as_ref()?.get("look")?.clone()).ok()
    }

    /// The fields what's held changes, in Edit Bot's order (others a newer box names, after).
    pub fn fields(&self) -> Vec<String> {
        let Some(change) = &self.change else { return Vec::new() };
        let known = [
            ("name", "name"), ("look", "look"), ("job", "job"), ("jobLine", "job"),
            ("jobDetails", "instructions"), ("personality", "personality"), ("model", "model"), ("effort", "effort"),
        ];
        let mut fields: Vec<String> = known.iter()
            .filter(|(key, _)| change.contains_key(*key))
            .map(|(_, label)| (*label).to_owned())
            .collect();
        fields.dedup();
        fields.extend(change.keys().filter(|field| !known.iter().any(|(key, _)| *key == field.as_str())).cloned());
        fields
    }
}

/// What a computer refused of an edit held for it, in the last day, with the words to show.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Refused {
    pub bot: String,
    #[serde(default)]
    pub computer: String,
    /// The bot's name and look, as a held edit has them.
    #[serde(default, rename = "name")]
    pub bot_name: Option<String>,
    #[serde(default, rename = "look", deserialize_with = "lenient")]
    pub bot_look: Option<Look>,
    pub message: String,
    #[serde(default)]
    pub at: Option<f64>,
}

/// A bot that lives on another of your computers, as the box asked says it for drawing
/// (`bots.elsewhere`): as that computer last told it, and as you left it (its name and look those
/// a change the box holds gives it).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Elsewhere {
    pub id: String,
    /// The computer it lives on, by its box key.
    #[serde(default, rename = "box")]
    pub computer_key: String,
    #[serde(default)]
    pub session: String,
    /// None: one the box has no name for (no computer of yours ever listed it to it).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub look: Option<Look>,
    /// The box holds a change or a delete for it.
    #[serde(default)]
    pub waiting: bool,
    /// Its computer refused a change, lately.
    #[serde(default)]
    pub refused: bool,
    /// No computer of yours lists it now: it's here only by what's held or was refused, and
    /// nothing more is taken for it.
    #[serde(default)]
    pub unlisted: bool,
    /// What it does, in a few words; empty where its computer said none.
    #[serde(default)]
    pub job: String,
    /// The name of the computer it lives on, as the box asked knows it.
    #[serde(default)]
    pub computer: String,
    /// What deleting it takes with it there, in the box's words.
    #[serde(default, deserialize_with = "readable")]
    pub deletes: Vec<DeleteLine>,
    /// Its computer isn't answering, as the box says it.
    #[serde(default)]
    pub away: bool,
    /// When its computer was last heard from (ms), while it's away.
    #[serde(default, rename = "lastSeen")]
    pub last_seen: Option<f64>,
    /// Its computer is only switching to a new release: not away, said to be updating.
    #[serde(default)]
    pub updating: bool,
    #[serde(default)]
    pub now: Option<crate::areas::BotNow>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub wakes: Option<f64>,
}

/// What `kuvo/bots` says: the bots that live on the box, those that live on your other computers,
/// and what it holds for (and heard back from) those computers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BotsSaid {
    pub pending_computers: Vec<crate::areas::SetupComputer>,
    pub bots: Vec<Bot>,
    pub elsewhere: Vec<Elsewhere>,
    pub held: Vec<Held>,
    pub refused: Vec<Refused>,
    /// The box couldn't say what it holds just now (it leaves `held` and `refused` out while it
    /// can't check its seals): a client keeps what it last heard ([`BotsSaid::keeping`]).
    pub held_unsaid: bool,
}

/// A line of `kuvo/bots/events` reads as the list does.
impl<'de> Deserialize<'de> for BotsSaid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let answer = Value::deserialize(deserializer)?;
        if !answer.get("bots").is_some_and(Value::is_array) {
            return Err(serde::de::Error::custom("the box didn’t list its bots"));
        }
        Ok(said(&answer))
    }
}

impl BotsSaid {
    /// This answer, with what `before` said the box holds where this one couldn't say.
    pub fn keeping(mut self, before: Option<&BotsSaid>) -> BotsSaid {
        if self.held_unsaid
            && let Some(before) = before
        {
            self.held = before.held.clone();
            self.refused = before.refused.clone();
            self.elsewhere = before.elsewhere.clone();
        }
        self
    }
}

/// One of New Bot's templates.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub job: String,
    #[serde(default, deserialize_with = "lenient")]
    pub look: Option<Look>,
    #[serde(default)]
    pub personality: Option<String>,
    /// It's made to work in a project (a fact a client may draw; the box decides what's needed).
    #[serde(default, rename = "wantsProject")]
    pub wants_project: bool,
}

/// A personality a bot can have.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Personality {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// One you made.
    #[serde(default)]
    pub custom: bool,
}

/// The looks a bot can have, by name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Looks {
    #[serde(default)]
    pub colors: Vec<String>,
    #[serde(default)]
    pub shapes: Vec<String>,
    #[serde(default)]
    pub faces: Vec<String>,
}

/// A project a new bot can work in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BotProject {
    pub directory: String,
    #[serde(default)]
    pub name: String,
}

/// What New Bot offers on a box (`kuvo/bots/templates`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Templates {
    #[serde(default, deserialize_with = "readable")]
    pub templates: Vec<Template>,
    #[serde(default, deserialize_with = "readable")]
    pub personalities: Vec<Personality>,
    #[serde(default)]
    pub looks: Looks,
    /// The model a new bot starts on there, by the place's name (one key).
    #[serde(default, rename = "startingModel")]
    pub starting_model: HashMap<String, Option<ModelRef>>,
    #[serde(default, deserialize_with = "readable")]
    pub projects: Vec<BotProject>,
}

impl Templates {
    /// The model a new bot starts on, where the box names one.
    pub fn starting(&self) -> Option<&ModelRef> {
        self.starting_model.values().next().and_then(Option::as_ref)
    }
}

/// A new bot, as `POST kuvo/bots` takes it. What's left out is the box's to pick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewBot {
    pub name: String,
    pub look: Option<Look>,
    pub personality: Option<String>,
    pub model: Option<ModelRef>,
    pub project: Option<String>,
    pub template: Option<String>,
    /// This New Bot's own id, sent again with a retry so the bot is made once.
    pub request: String,
}

/// A bot the box made: the bot, and (from a template) its job for your composer, to edit and send.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Made {
    pub bot: Bot,
    #[serde(default, rename = "firstMessage")]
    pub first_message: Option<String>,
}

/// A change to a bot, as `PATCH kuvo/bots/<id>` takes it: only the fields given.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BotChange {
    pub name: Option<String>,
    pub look: Option<Look>,
    /// `Some(None)`: the model's own way.
    pub personality: Option<Option<String>>,
    pub model: Option<ModelRef>,
    /// The effort its model runs at (`bots.effort`); `Some(None)`: the model's own.
    pub effort: Option<Option<String>>,
    /// Its line and its details, the whole job.
    pub job: Option<(String, String)>,
    pub job_line: Option<String>,
    pub job_details: Option<String>,
}

impl BotChange {
    fn body(&self) -> Value {
        let mut body = Map::new();
        if let Some(name) = &self.name {
            body.insert("name".into(), json!(name));
        }
        if let Some(look) = &self.look {
            body.insert("look".into(), look_json(look));
        }
        if let Some(personality) = &self.personality {
            body.insert("personality".into(), json!(personality));
        }
        if let Some(model) = &self.model {
            body.insert("model".into(), json!(model));
        }
        if let Some(effort) = &self.effort {
            body.insert("effort".into(), json!(effort));
        }
        if let Some((line, details)) = &self.job {
            body.insert("job".into(), json!({ "line": line, "details": details }));
        }
        if let Some(line) = &self.job_line {
            body.insert("jobLine".into(), json!(line));
        }
        if let Some(details) = &self.job_details {
            body.insert("jobDetails".into(), json!(details));
        }
        Value::Object(body)
    }
}

fn look_json(look: &Look) -> Value {
    json!({ "color": look.color, "shape": look.shape, "face": look.face })
}

/// What came of a change or a delete.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Done where the bot lives: the bot as it is now (none: deleted, or it no longer lists here).
    Done(Option<Box<Bot>>),
    /// Taken, and held for the computer it lives on (`202`).
    Held(Box<Held>),
}

/// How one of a bot's sessions stands, in the box's word. One a newer box names is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkState {
    Working,
    Asking,
    Finished,
    Failed,
    Stopped,
    Idle,
    #[default]
    #[serde(other)]
    Other,
}

/// One of a bot's sessions (`kuvo/work?bot=`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkSession {
    pub session: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub state: WorkState,
    /// When it started (ms).
    #[serde(default)]
    pub started: Option<f64>,
    /// The model it runs on, and its effort.
    #[serde(default, deserialize_with = "lenient")]
    pub model: Option<WorkModel>,
    /// Its mode (`plan`, `build`).
    #[serde(default)]
    pub mode: Option<String>,
    /// The account it runs on, as Settings › Accounts lists it.
    #[serde(default, deserialize_with = "lenient")]
    pub account: Option<WorkAccount>,
    /// What it asks you, while it waits on you.
    #[serde(default)]
    pub asks: Option<String>,
}

impl WorkSession {
    /// At work: working, or waiting on you.
    pub fn is_at_work(&self) -> bool {
        matches!(self.state, WorkState::Working | WorkState::Asking)
    }
}

/// A model as a bot's work names it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkModel {
    pub id: String,
    #[serde(default, alias = "providerID")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
}

/// An account as a bot's work names it: the box's whole short name for it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkAccount {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// What a bot started today, and the tokens its sessions used.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct WorkToday {
    #[serde(default)]
    pub started: u32,
    /// The asks it answered for its sessions since midnight on its computer.
    #[serde(default)]
    pub answered: u32,
    #[serde(default)]
    pub tokens: u64,
}

/// When each bot's work last changed (`kuvo/work/events`), by its id.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct WorkChanged {
    #[serde(default)]
    pub changed: std::collections::BTreeMap<String, f64>,
}

/// A bot's sessions, the latest first.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Work {
    pub activity: Option<crate::background::Activity>,
    #[serde(default, deserialize_with = "readable")]
    pub sessions: Vec<WorkSession>,
    #[serde(default)]
    pub today: WorkToday,
}

/// A value a newer box may say in a way this doesn't know: none.
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Option<T>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

/// What a `kuvo/bots` answer says. `held` and `refused` left out: not known now, not none.
fn said(answer: &Value) -> BotsSaid {
    BotsSaid { pending_computers: read_all(answer.get("pendingComputers")), bots: read_all(answer.get("bots")), elsewhere: read_all(answer.get("elsewhere")), held: read_all(answer.get("held")), refused: read_all(answer.get("refused")), held_unsaid: answer.get("held").is_none() }
}

fn read_all<T: serde::de::DeserializeOwned>(values: Option<&Value>) -> Vec<T> {
    values.and_then(Value::as_array).map(|values| values.iter().filter_map(|value| serde_json::from_value(value.clone()).ok()).collect()).unwrap_or_default()
}

impl BoxClient {
    /// The box's bots, with what it holds for your other computers and what they refused.
    pub fn bots_said(&self) -> Result<BotsSaid> {
        let answer: Value = self.get("kuvo/bots")?;
        if !answer.get("bots").is_some_and(Value::is_array) {
            return Err(Error::BadAnswer("The box didn’t list its bots.".into()));
        }
        Ok(said(&answer))
    }

    /// What New Bot offers on this box.
    pub fn bot_templates(&self) -> Result<Templates> {
        self.get("kuvo/bots/templates")
    }

    /// Changes a custom personality in the shared library.
    pub fn change_personality(&self, id: &str, name: &str, description: &str) -> Result<Personality> {
        let mut changed: Personality = self.value(&Request {
            method: "PATCH".into(),
            path: format!("kuvo/personalities/{}", crate::paths::segment(id)),
            headers: Vec::new(),
            body: Some(json!({ "name": name, "description": description })),
        })?;
        // The library route returns a custom record without the templates catalogue's marker.
        changed.custom = true;
        Ok(changed)
    }

    /// Makes a bot on this box.
    pub fn make_bot(&self, new: &NewBot) -> Result<Made> {
        let mut body = Map::new();
        body.insert("name".into(), json!(new.name));
        body.insert("request".into(), json!(new.request));
        if let Some(look) = &new.look {
            body.insert("look".into(), look_json(look));
        }
        for (field, value) in [("personality", &new.personality), ("project", &new.project), ("template", &new.template)] {
            if let Some(value) = value {
                body.insert(field.into(), json!(value));
            }
        }
        if let Some(model) = &new.model {
            body.insert("model".into(), json!(model));
        }
        self.value(&Request::post("kuvo/bots", Value::Object(body)))
    }

    /// Changes a bot. Asked of a box it doesn't live on, the box holds it for that computer.
    pub fn change_bot(&self, bot: &str, change: &BotChange) -> Result<Outcome> {
        let request = Request { method: "PATCH".into(), path: format!("kuvo/bots/{bot}"), headers: Vec::new(), body: Some(change.body()) };
        self.bot_outcome(request)
    }

    /// Deletes a bot, as Delete Bot… does. Asked of a box it doesn't live on, it's held too.
    pub fn delete_bot(&self, bot: &str) -> Result<Outcome> {
        self.bot_outcome(Request::delete(format!("kuvo/bots/{bot}")))
    }

    fn bot_outcome(&self, request: Request) -> Result<Outcome> {
        #[derive(Deserialize)]
        struct Answer {
            #[serde(default, deserialize_with = "lenient")]
            bot: Option<Bot>,
            #[serde(default, deserialize_with = "lenient")]
            held: Option<Held>,
        }
        let response = self.send(&request)?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        let answer: Answer = decode(&response)?;
        Ok(match (response.status, answer.held) {
            (202, Some(held)) => Outcome::Held(Box::new(held)),
            _ => Outcome::Done(answer.bot.map(Box::new)),
        })
    }

    /// A bot's sessions, the latest first.
    pub fn bot_work(&self, bot: &str) -> Result<Work> {
        self.get(&format!("kuvo/work?bot={bot}"))
    }

    /// When each bot's work last changed, now and as it changes.
    pub fn follow_work_changes(&self) -> crate::Follow<crate::follow::Followed<WorkChanged>> {
        crate::follow::follow_lines(self, "kuvo/work/events".into())
    }

    /// The box's bots followed: the list now, and again as they change (made, changed, deleted,
    /// and what each is doing: `bots.now`, its `state` too).
    pub fn follow_bots(&self) -> crate::Follow<crate::follow::Followed<BotsSaid>> {
        // A box from before Bots events can still answer its list.
        crate::follow::follow_lines_filled(self, "kuvo/bots/events".into(), |said| said)
    }

    /// You've seen what a bot did (its conversation shown): the box reads its sessions that ended
    /// since you last did, so it's no longer done or blocked on any client (`bots.state`).
    pub fn bot_seen(&self, bot: &str) -> Result<()> {
        let response = self.send(&Request::post(format!("kuvo/bots/{}/seen", crate::paths::segment(bot)), serde_json::json!({})))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        Ok(())
    }

    /// Every session of a bot's work stopped (Stop All).
    pub fn stop_bot_work(&self, bot: &str) -> Result<()> {
        let response = self.send(&Request::post(format!("kuvo/bots/{}/stop-all", crate::paths::segment(bot)), serde_json::json!({})))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_held_and_refused_reads_as_the_box_says_it() {
        let held: Held = serde_json::from_str(r#"{"bot":"bot_scout","box":"key_studio","computer":"studio-mini","change":{"name":"Scout II","look":{"color":"amber","shape":"gem","face":"wink"},"mood":"new"},"since":5}"#).unwrap();
        assert_eq!((held.name(), held.look().map(|look| look.color), held.delete), (Some("Scout II"), Some("amber".into()), false));
        assert_eq!((held.bot_name.as_deref(), &held.bot_look), (None, &None), "a bot no computer listed to the box: neither");
        assert_eq!(held.fields(), vec!["name", "look", "mood"], "Edit Bot's order, then what a newer box names");
        let delete: Held = serde_json::from_str(r#"{"bot":"bot_scout","box":"","computer":"old-box","delete":true,"since":5}"#).unwrap();
        assert!(delete.delete && delete.fields().is_empty());
        let refused: Refused = serde_json::from_str(r#"{"bot":"bot_scout","box":"k","computer":"studio-mini","message":"studio-mini has no personality “Dry Wit”.","at":9}"#).unwrap();
        assert_eq!(refused.message, "studio-mini has no personality “Dry Wit”.");
    }

    /// PROTOCOL.md, `kuvo/bots`: what deleting a bot takes with it, the bots of your other
    /// computers, and who a held edit is for, each as the box says it.
    #[test]
    fn deletes_and_the_bots_elsewhere_read_as_the_box_says_them() {
        let look = json!({ "color": "amber", "shape": "gem", "face": "wink" });
        let deletes = json!([{ "what": "folder", "words": "Its folder goes to the Trash." }, { "what": "a newer box's", "words": "Something new.", "kept": true }, { "what": "no words" }]);
        let said = said(&json!({
            "bots": [{ "id": "bot_mira", "session": "ses_mira", "name": "Mira", "look": look, "deletes": deletes }],
            "elsewhere": [{ "id": "bot_scout", "session": "ses_scout", "name": "Scout", "look": look, "job": "Watches CI", "box": "k", "computer": "studio-mini", "kind": "mac", "deletes": deletes, "waiting": true }, { "id": "bot_wren", "session": "ses_wren", "name": "Wren", "look": "striped", "box": "k2", "computer": "lab", "kind": "machine", "deletes": [], "refused": true }, { "id": "bot_ghost", "box": "", "computer": "old-box", "waiting": true, "unlisted": true }],
            "held": [{ "bot": "bot_scout", "box": "k", "computer": "studio-mini", "name": "Scout", "look": look, "delete": true, "since": 5 }],
            "refused": [{ "bot": "bot_wren", "box": "k2", "computer": "lab", "name": "Wren", "message": "No.", "at": 9 }],
        }));
        let lines: Vec<(&str, bool)> = said.bots[0].deletes.iter().map(|line| (line.words.as_str(), line.kept)).collect();
        assert_eq!(lines, vec![("Its folder goes to the Trash.", false), ("Something new.", true)], "a line without words is left out; one of a kind this doesn't know is a line all the same");
        assert_eq!(said.elsewhere.iter().map(|bot| (bot.name.as_deref(), bot.computer.as_str(), bot.job.as_str(), bot.look.is_some(), bot.deletes.len())).collect::<Vec<_>>(), vec![(Some("Scout"), "studio-mini", "Watches CI", true, 2), (Some("Wren"), "lab", "", false, 0), (None, "old-box", "", false, 0)]);
        assert_eq!(said.elsewhere.iter().map(|bot| (bot.waiting, bot.refused, bot.unlisted)).collect::<Vec<_>>(), vec![(true, false, false), (false, true, false), (true, false, true)]);
        assert_eq!((said.held[0].bot_name.as_deref(), said.held[0].bot_look.as_ref().map(|look| look.color.as_str())), (Some("Scout"), Some("amber")));
        assert_eq!((said.refused[0].bot_name.as_deref(), &said.refused[0].bot_look), (Some("Wren"), &None));
    }

    /// PROTOCOL.md, "Held for another computer": neither is there while the box can't check its
    /// seals, and a client keeps what it last heard.
    #[test]
    fn whats_held_is_kept_while_the_box_cant_say() {
        let held = json!({ "bot": "bot_scout", "box": "k", "computer": "studio-mini", "change": { "name": "Scout" }, "since": 5 });
        let before = said(&json!({ "bots": [], "held": [held], "refused": [] }));
        assert!(!before.held_unsaid && before.held.len() == 1);
        let unsaid = said(&json!({ "bots": [] })).keeping(Some(&before));
        assert_eq!(unsaid.held, before.held, "left out: what was last heard stays");
        let there = said(&json!({ "bots": [], "held": [], "refused": [], "elsewhere": [{ "id": "bot_scout", "name": "Scout", "box": "k", "computer": "studio-mini" }] }));
        assert_eq!(said(&json!({ "bots": [] })).keeping(Some(&there)).elsewhere, there.elsewhere, "and the bots elsewhere with it");
        let none = said(&json!({ "bots": [], "held": [], "refused": [] })).keeping(Some(&before));
        assert!(none.held.is_empty(), "said, and empty: it went through");
    }

    #[test]
    fn a_change_sends_only_the_fields_given() {
        assert_eq!(BotChange { name: Some("Mira".into()), ..BotChange::default() }.body(), json!({ "name": "Mira" }));
        assert_eq!(BotChange { personality: Some(None), ..BotChange::default() }.body(), json!({ "personality": null }), "null: the model's own way");
        assert_eq!(BotChange { job: Some(("Ships betas".into(), String::new())), ..BotChange::default() }.body(), json!({ "job": { "line": "Ships betas", "details": "" } }));
        assert_eq!(BotChange { job_line: Some("Ships betas".into()), ..BotChange::default() }.body(), json!({ "jobLine": "Ships betas" }));
        assert_eq!(BotChange { job_details: Some(String::new()), ..BotChange::default() }.body(), json!({ "jobDetails": "" }));
        let held: Held = serde_json::from_value(json!({
            "bot": "bot_mira", "box": "k", "computer": "lab",
            "change": { "job": { "line": "Old job" }, "jobLine": "New job", "jobDetails": "" }, "since": 5
        })).unwrap();
        assert_eq!(held.fields(), vec!["job", "instructions"]);
        let model = ModelRef { provider_id: "anthropic".into(), id: "claude-opus-5-5".into(), variant: None };
        assert_eq!(BotChange { model: Some(model), ..BotChange::default() }.body(), json!({ "model": { "providerID": "anthropic", "id": "claude-opus-5-5" } }));
        assert_eq!(BotChange { effort: Some(Some("high".into())), ..BotChange::default() }.body(), json!({ "effort": "high" }));
        assert_eq!(BotChange { effort: Some(None), ..BotChange::default() }.body(), json!({ "effort": null }), "null: the model's own");
    }

    #[test]
    fn templates_and_work_leave_out_what_cant_be_read() {
        let said: Templates = serde_json::from_str(
            r#"{"templates":[{"id":"news","name":"Herald","job":"Reads the news","instructions":"…","look":{"color":"sky","shape":"round","face":"calm"},"wantsProject":false},{"id":"ci","name":"Keeper","look":"striped","personality":"preset.perfectionist","wantsProject":true},{"name":"No id"}],
                "personalities":[{"id":"preset.straight","name":"Straight Shooter","description":"Says it plainly.","custom":false}],
                "looks":{"colors":["mint"],"shapes":["bloom"],"faces":["wink"]},"startingModel":{"mac":{"providerID":"anthropic","id":"claude-opus-5-5","variant":"high"}},"projects":[{"directory":"/p/site","name":"Website"}]}"#,
        )
        .unwrap();
        assert_eq!(said.templates.len(), 2);
        assert!(said.templates[1].look.is_none() && said.templates[1].wants_project, "a look this can't read is none, not the template");
        assert_eq!(said.starting().map(|model| model.id.as_str()), Some("claude-opus-5-5"));
        let work: Work = serde_json::from_str(r#"{"bot":"bot_1","sessions":[{"session":"ses_1","title":"Fix it","folder":"/p","state":"asking","started":4,"tokens":1},{"session":"ses_2","state":"dozing"},{"title":"no id"}]}"#).unwrap();
        assert_eq!(work.sessions.iter().map(|session| session.state).collect::<Vec<_>>(), vec![WorkState::Asking, WorkState::Other]);
    }
}
