//! The Work Queue as a client shows it (PROTOCOL.md, "The Work Queue"): the view across your
//! computers (`kuvo/work-queue?owner=all&counts=1`: its statuses, priorities, labels and who an item
//! may be for, its items in the box's order, who owns them, what's deleted, your other computers and
//! the box's counts), the same view narrowed by a search (`q`), one item whole
//! (`kuvo/work-queue/items/<id>`), and what you do: add an item, change what one says and where it
//! stands, delete it, bring it back, and make, change and remove labels. Everything is read as the
//! box says it; an item in a shape this can't read is left out, not the list.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::areas::Look;
use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::paths;
use crate::transport::Request;

/// A status, as the box lists them, in the order a client shows them; `closed` for those an item
/// is finished in (Done, Canceled).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Priority {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A label: what kind of work an item is. `color` is one of the looks' names.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Label {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: String,
}

/// Who an item is, or may be, for: a bot (with its `look`, where it's a bot of the box asked) or a
/// coding agent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Assignee {
    #[serde(default)]
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "look_or_none")]
    pub look: Option<Look>,
}

/// (A look in a shape this can't read is no look, not no assignee.)
fn look_or_none<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<Look>, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(deserializer)?).ok())
}

/// What whoever asked may do to an item, as the box says: nothing it leaves out is offered.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Can {
    /// Change its title and description.
    #[serde(default)]
    pub edit: bool,
    /// Change where it stands among the open statuses, its priority, labels and due day.
    #[serde(default)]
    pub set: bool,
    /// Say who it's for, among the view's `assignees` (the bots of the box asked: an item another
    /// computer keeps is assigned there).
    #[serde(default)]
    pub assign: bool,
    /// Close it: Done or Canceled.
    #[serde(default)]
    pub close: bool,
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub restore: bool,
    /// Where something can't be done, why, in the box's words: shown as they are.
    #[serde(default)]
    pub why: Option<String>,
}

/// What an item added with nothing said starts as: a status and a priority, by their ids.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Starts {
    pub status: String,
    pub priority: String,
}

/// A view's counts, as the box counted them from the items it lists (those a search left): how many
/// are open, of each status, priority and label (by its id), from each outside source (by its
/// kind), on each computer (by its key), and how many are deleted.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub open: usize,
    #[serde(default)]
    pub statuses: HashMap<String, usize>,
    #[serde(default)]
    pub priorities: HashMap<String, usize>,
    #[serde(default)]
    pub labels: HashMap<String, usize>,
    #[serde(default)]
    pub sources: HashMap<String, usize>,
    #[serde(default)]
    pub computers: HashMap<String, usize>,
    #[serde(default)]
    pub removed: usize,
}

/// Who did something (`created.by`, a note's `by`): you, a session by its name, or another shape
/// kept as it came (an outside tracker's), shown by its `name`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Who {
    pub user: bool,
    pub name: Option<String>,
}

impl<'de> Deserialize<'de> for Who {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Who, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Who { user: value.get("user").and_then(Value::as_bool).unwrap_or(false), name: value.get("name").and_then(Value::as_str).map(str::to_owned).filter(|name| !name.trim().is_empty()) })
    }
}

/// When something was done, and by whom.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Stamp {
    #[serde(default)]
    pub at: f64,
    #[serde(default)]
    pub by: Who,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Note {
    #[serde(default)]
    pub at: f64,
    #[serde(default)]
    pub by: Who,
    #[serde(default)]
    pub text: String,
}

/// How far an item is, and what's being done now.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Progress {
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub note: Option<String>,
}

/// A session working on an item, and whether it's at work now.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ItemSession {
    pub session: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub working: bool,
}

/// An outside source's item (an issue or a pull request): `name` is what's shown in place of `#n`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub kind: String,
    #[serde(default, rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Where its issue or pull request stands on GitHub, as last read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Seen {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub gone: bool,
}

/// Another of your computers: the one an item is on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct There {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
}

/// A computer that told of its queue: as of when, and since when it hasn't answered.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Told {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub at: f64,
    #[serde(default)]
    pub away: Option<f64>,
}

/// Where an item is on its way to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Handing {
    #[serde(default)]
    pub name: String,
}

/// An item as a list shows it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub number: Option<u64>,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub title: String,
    /// Where it stands: one of the view's statuses, by its id.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: String,
    /// Its labels, by their ids.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Who it's for.
    #[serde(default)]
    pub assignee: Option<Assignee>,
    /// The day it's due, `YYYY-MM-DD`.
    #[serde(default)]
    pub due: Option<String>,
    /// It's late, as the box says: its day is gone by and it's still open.
    #[serde(default)]
    pub overdue: bool,
    /// What may be done to it (nothing, where the box says nothing).
    #[serde(default)]
    pub can: Can,
    #[serde(default)]
    pub progress: Option<Progress>,
    #[serde(default)]
    pub sessions: Vec<ItemSession>,
    #[serde(default, rename = "noteCount")]
    pub note_count: usize,
    #[serde(default, rename = "fileCount")]
    pub file_count: usize,
    #[serde(default, rename = "linkCount")]
    pub link_count: usize,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub seen: Option<Seen>,
    /// Its title is drawn struck through (closed without being done, or gone).
    #[serde(default)]
    pub struck: bool,
    /// What's shown in place of its status while GitHub makes it Done.
    #[serde(default, rename = "doneAs")]
    pub done_as: Option<String>,
    #[serde(default)]
    pub handing: Option<Handing>,
    #[serde(default)]
    pub computer: Option<There>,
    #[serde(default)]
    pub created: Stamp,
    #[serde(default)]
    pub updated: Stamp,
    #[serde(default, rename = "doneAt")]
    pub done_at: Option<f64>,
    #[serde(default)]
    pub removed: Option<Stamp>,
}

impl Item {
    /// Whether a session is at work on it now.
    pub fn at_work(&self) -> bool {
        self.sessions.iter().any(|session| session.working)
    }

    /// What names it where a number would: `#243`, an issue's own name, or nothing while it waits
    /// for its number.
    pub fn label(&self) -> Option<String> {
        match (&self.source, self.number) {
            (Some(source), _) => Some(source.name.clone().filter(|name| !name.is_empty()).unwrap_or_else(|| source.reference.clone())),
            (None, Some(number)) => Some(format!("#{number}")),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct File {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Link {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// An item whole: what a list shows, and what it says, its notes, files and links.
#[derive(Debug, Clone, PartialEq)]
pub struct Whole {
    pub item: Item,
    /// On another computer, as that one last told it: without what only it keeps.
    pub told: bool,
    pub body: String,
    pub notes: Vec<Note>,
    pub files: Vec<File>,
    pub links: Vec<Link>,
}

/// An owner in a view: what it is, and its title.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Owner {
    pub owner: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub bot: Option<OwnerBot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OwnerBot {
    #[serde(default)]
    pub name: String,
}

impl Owner {
    /// The owner by name: the bot it is, else its conversation's title.
    pub fn name(&self) -> Option<String> {
        self.bot.as_ref().map(|bot| bot.name.clone()).or_else(|| self.title.clone()).filter(|name| !name.trim().is_empty())
    }
}

/// The Work Queue across your computers, as the box says it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueueSaid {
    /// The statuses, in the order the box shows them.
    pub statuses: Vec<Status>,
    pub priorities: Vec<Priority>,
    /// The labels, in the order they were made.
    pub labels: Vec<Label>,
    /// Who an item may be for.
    pub assignees: Vec<Assignee>,
    /// What an item added with nothing said starts as.
    pub new: Option<Starts>,
    /// The day it is on the box asked (`YYYY-MM-DD`): what every day here is counted from.
    pub today: String,
    /// The items, in the box's order.
    pub items: Vec<Item>,
    pub owners: Vec<Owner>,
    /// What's deleted, newest first.
    pub removed: Vec<Item>,
    /// Your other computers that told of their queues.
    pub computers: Vec<Told>,
    /// The box's counts, from this same view.
    pub counts: Counts,
    /// The words it was narrowed by, as the box says them back.
    pub q: Option<String>,
}

/// What a view is narrowed by, all of it the box's to apply: words to search for (`q`), and its
/// filters, each a list of which any one may match: computers (by key; empty for the box asked),
/// outside sources (by kind), owners, and labels (by id).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Narrow {
    pub q: String,
    pub computers: Vec<String>,
    pub sources: Vec<String>,
    pub owners: Vec<String>,
    pub labels: Vec<String>,
}

impl Narrow {
    pub fn is_empty(&self) -> bool {
        *self == Narrow::default()
    }

    /// The view's route with what narrows it: `…&q=…&computers=a,b&sources=…&owners=…&labels=…`.
    pub fn route(&self) -> String {
        let mut route = "kuvo/work-queue?owner=all&counts=1".to_owned();
        if !self.q.is_empty() {
            route.push_str(&format!("&q={}", paths::query_value(&self.q)));
        }
        for (name, list) in [("computers", &self.computers), ("sources", &self.sources), ("owners", &self.owners), ("labels", &self.labels)] {
            if !list.is_empty() {
                route.push_str(&format!("&{name}={}", list.iter().map(|each| paths::query_value(each)).collect::<Vec<_>>().join(",")));
            }
        }
        route
    }
}

/// A new item, as written: what's left out is the box's to choose.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewItem {
    pub title: String,
    pub body: String,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    /// Who it's for: a kind (`bot`, `agent`) and an id.
    pub assignee: Option<(String, String)>,
    pub due: Option<String>,
}

/// A change to an item: only what's said changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemChange {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    /// All of its labels, in place of those it had.
    pub labels: Option<Vec<String>>,
    /// Who it's for (a kind and an id); `Some(None)` for no one.
    pub assignee: Option<Option<(String, String)>>,
    /// The day it's due; `Some(None)` for none.
    pub due: Option<Option<String>>,
}

impl ItemChange {
    fn body(&self) -> Value {
        let mut body = Map::new();
        let mut put = |name: &str, value: Option<Value>| {
            if let Some(value) = value {
                body.insert(name.into(), value);
            }
        };
        put("title", self.title.as_ref().map(|title| json!(title)));
        put("body", self.body.as_ref().map(|body| json!(body)));
        put("status", self.status.as_ref().map(|status| json!(status)));
        put("priority", self.priority.as_ref().map(|priority| json!(priority)));
        put("labels", self.labels.as_ref().map(|labels| json!(labels)));
        put("assignee", self.assignee.as_ref().map(|assignee| assignee.as_ref().map_or(Value::Null, |(kind, id)| json!({ "kind": kind, "id": id }))));
        put("due", self.due.as_ref().map(|due| due.as_ref().map_or(Value::Null, |due| json!(due))));
        Value::Object(body)
    }
}

fn read_all<T: serde::de::DeserializeOwned>(values: Option<&Value>) -> Vec<T> {
    values.and_then(Value::as_array).map(|values| values.iter().filter_map(|value| serde_json::from_value(value.clone()).ok()).collect()).unwrap_or_default()
}

fn item_route(item: &str) -> String {
    format!("kuvo/work-queue/items/{}", paths::segment(item))
}

impl BoxClient {
    /// Every queue on the box and on each other computer that told of its own, with the box's
    /// counts: what the Work Queue itself shows.
    pub fn work_queue_view(&self) -> Result<QueueSaid> {
        self.queue_view("kuvo/work-queue?owner=all&counts=1")
    }

    /// The same view narrowed as `by` says (a search, filters), by the box: only the items it
    /// leaves, and its counts of those.
    pub fn work_queue_narrowed(&self, by: &Narrow) -> Result<QueueSaid> {
        self.queue_view(&by.route())
    }

    fn queue_view(&self, path: &str) -> Result<QueueSaid> {
        let answer: Value = self.get(path)?;
        if !answer.get("items").is_some_and(Value::is_array) {
            return Err(Error::BadAnswer("The box didn’t list its Work Queue.".into()));
        }
        Ok(QueueSaid {
            statuses: read_all(answer.get("statuses")),
            priorities: read_all(answer.get("priorities")),
            labels: read_all(answer.get("labels")),
            assignees: read_all(answer.get("assignees")),
            new: answer.get("new").cloned().and_then(|new| serde_json::from_value(new).ok()),
            today: answer.get("today").and_then(Value::as_str).unwrap_or_default().to_owned(),
            items: read_all(answer.get("items")),
            owners: read_all(answer.get("owners")),
            removed: read_all(answer.get("removed")),
            computers: read_all(answer.get("computers")),
            counts: answer.get("counts").cloned().and_then(|counts| serde_json::from_value(counts).ok()).unwrap_or_default(),
            q: answer.get("q").and_then(Value::as_str).map(str::to_owned),
        })
    }

    /// An item whole, by its id.
    pub fn work_item(&self, item: &str) -> Result<Whole> {
        let answer: Value = self.get(&item_route(item))?;
        let said = answer.get("item").cloned().unwrap_or(Value::Null);
        let item: Item = serde_json::from_value(said.clone()).map_err(|error| Error::BadAnswer(format!("The item couldn't be read: {error}")))?;
        Ok(Whole {
            item,
            told: said.get("told").and_then(Value::as_bool).unwrap_or(false),
            body: said.get("body").and_then(Value::as_str).unwrap_or_default().to_owned(),
            notes: read_all(said.get("notes")),
            files: read_all(said.get("files")),
            links: read_all(said.get("links")),
        })
    }

    /// Adds an item. Whose queue it goes in, and what's left out, are the box's to decide.
    pub fn add_work_item(&self, new: &NewItem) -> Result<Item> {
        let mut body = Map::new();
        body.insert("title".into(), json!(new.title));
        if !new.body.is_empty() {
            body.insert("body".into(), json!(new.body));
        }
        if let Some(status) = &new.status {
            body.insert("status".into(), json!(status));
        }
        if let Some(priority) = &new.priority {
            body.insert("priority".into(), json!(priority));
        }
        if !new.labels.is_empty() {
            body.insert("labels".into(), json!(new.labels));
        }
        if let Some((kind, id)) = &new.assignee {
            body.insert("assignee".into(), json!({ "kind": kind, "id": id }));
        }
        if let Some(due) = &new.due {
            body.insert("due".into(), json!(due));
        }
        self.item_answer(Request::post("kuvo/work-queue/items", Value::Object(body)))
    }

    /// Changes an item: what it says, where it stands, its priority, labels, assignee or due day.
    pub fn change_work_item(&self, item: &str, change: &ItemChange) -> Result<Item> {
        self.item_answer(Request { method: "PATCH".into(), path: item_route(item), headers: Vec::new(), body: Some(change.body()) })
    }

    /// Deletes an item (the box keeps it a while, to bring back).
    pub fn delete_work_item(&self, item: &str) -> Result<Item> {
        self.item_answer(Request::delete(item_route(item)))
    }

    /// Brings a deleted item back.
    pub fn restore_work_item(&self, item: &str) -> Result<Item> {
        self.item_answer(Request::post(format!("{}/restore", item_route(item)), json!({})))
    }

    /// Makes a label called `name`, in `color` (the box's choice where none is said). Answers the
    /// labels as they are now, and the one made.
    pub fn add_label(&self, name: &str, color: Option<&str>) -> Result<(Vec<Label>, Option<Label>)> {
        let mut body = json!({ "add": name });
        if let Some(color) = color {
            body["color"] = json!(color);
        }
        self.labels_answer(body)
    }

    /// Renames or recolours a label (by its id): only what's said changes.
    pub fn change_label(&self, label: &str, name: Option<&str>, color: Option<&str>) -> Result<(Vec<Label>, Option<Label>)> {
        let mut body = json!({ "change": label });
        if let Some(name) = name {
            body["name"] = json!(name);
        }
        if let Some(color) = color {
            body["color"] = json!(color);
        }
        self.labels_answer(body)
    }

    /// Removes a label (by its id): the box takes it off every item first.
    pub fn remove_label(&self, label: &str) -> Result<(Vec<Label>, Option<Label>)> {
        self.labels_answer(json!({ "remove": label }))
    }

    fn labels_answer(&self, body: Value) -> Result<(Vec<Label>, Option<Label>)> {
        let answer: Value = self.value(&Request::post("kuvo/work-queue/labels", body))?;
        if !answer.get("labels").is_some_and(Value::is_array) {
            return Err(Error::BadAnswer("The box didn’t list its labels.".into()));
        }
        Ok((read_all(answer.get("labels")), answer.get("label").cloned().and_then(|label| serde_json::from_value(label).ok())))
    }

    fn item_answer(&self, request: Request) -> Result<Item> {
        let response = self.send(&request)?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        let answer: Value = serde_json::from_slice(&response.body).map_err(|error| Error::BadAnswer(format!("The answer couldn't be read: {error}")))?;
        serde_json::from_value(answer.get("item").cloned().unwrap_or(Value::Null)).map_err(|error| Error::BadAnswer(format!("The item couldn't be read: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_reads_as_the_box_says_it_and_is_named_by_its_number_or_its_issue() {
        let item: Item = serde_json::from_str(
            r#"{"id":"wqi_1","number":243,"owner":"ses_a","title":"Fix it","status":"in-progress","order":1,"priority":"high","labels":["lbl_bug","lbl_web"],
            "assignee":{"kind":"bot","id":"bot_mira","name":"Mira","look":{"color":"sky","shape":"round","face":"calm"}},"due":"2026-11-30","overdue":true,"can":{"edit":true,"set":true,"assign":true,"close":true,"delete":true,"restore":false,"later":true},
            "progress":{"value":0.4,"note":"Writing the test"},"sessions":[{"session":"ses_a","title":"Fixing","working":true}],"noteCount":2,"fileCount":1,"linkCount":0,
            "created":{"at":5,"by":{"user":true}},"updated":{"at":9,"by":{"session":"ses_a","name":"Mira"}},"newer":{"kept":true}}"#,
        )
        .unwrap();
        assert_eq!(item.label().as_deref(), Some("#243"));
        assert!(item.at_work() && item.created.by.user && item.overdue);
        assert_eq!((item.status.as_str(), item.priority.as_str(), item.labels.len(), item.due.as_deref()), ("in-progress", "high", 2, Some("2026-11-30")));
        assert_eq!(item.assignee.as_ref().map(|assignee| (assignee.kind.as_str(), assignee.name.as_str(), assignee.look.as_ref().map(|look| look.color.as_str()))), Some(("bot", "Mira", Some("sky"))));
        assert_eq!(item.can, Can { edit: true, set: true, assign: true, close: true, delete: true, restore: false, why: None });
        assert_eq!((item.updated.by.name.as_deref(), item.progress.as_ref().and_then(|progress| progress.value)), (Some("Mira"), Some(0.4)));

        // An issue is named by GitHub's name for it, never its old number; one waiting for a number
        // has none. One that says no `can` (as another computer told it) may have nothing done to it.
        let issue: Item = serde_json::from_str(r#"{"id":"wqi_2","number":7,"title":"Crash","status":"done","priority":"normal","labels":[],"source":{"kind":"github","ref":"gh:acme/site#12","name":"site#12"},"struck":true,"doneAs":"Closed on GitHub","can":{"edit":false,"set":false,"close":false,"delete":true,"restore":false,"why":"It's closed on GitHub."},"created":{"at":1,"by":{"source":"github","name":"octo"}}}"#).unwrap();
        assert_eq!((issue.label().as_deref(), issue.struck, issue.done_as.as_deref(), issue.created.by.name.as_deref()), (Some("site#12"), true, Some("Closed on GitHub"), Some("octo")));
        assert_eq!((issue.can.delete, issue.can.why.as_deref()), (true, Some("It's closed on GitHub.")));
        let waiting: Item = serde_json::from_str(r#"{"id":"wqi_3","title":"New"}"#).unwrap();
        assert_eq!((waiting.label(), waiting.can), (None, Can::default()));
    }

    #[test]
    fn a_view_is_narrowed_by_what_the_route_says() {
        assert_eq!(Narrow::default().route(), "kuvo/work-queue?owner=all&counts=1");
        // The box asked is the empty key among computers: said, where another is beside it.
        let by = Narrow { q: "sign up".into(), computers: vec![String::new(), "key_1".into()], sources: vec!["github-pr".into()], owners: vec![], labels: vec!["lbl_a".into(), "lbl_b".into()] };
        assert_eq!(by.route(), "kuvo/work-queue?owner=all&counts=1&q=sign%20up&computers=,key_1&sources=github-pr&labels=lbl_a,lbl_b");
        assert!(Narrow::default().is_empty() && !by.is_empty());
    }

    #[test]
    fn a_change_sends_only_what_it_says_and_null_to_clear() {
        assert_eq!(ItemChange { status: Some("done".into()), ..ItemChange::default() }.body(), json!({ "status": "done" }));
        let change = ItemChange { labels: Some(vec![]), assignee: Some(None), due: Some(None), ..ItemChange::default() };
        assert_eq!(change.body(), json!({ "labels": [], "assignee": null, "due": null }));
        let change = ItemChange { assignee: Some(Some(("agent".into(), "codex-cli".into()))), due: Some(Some("2026-12-01".into())), ..ItemChange::default() };
        assert_eq!(change.body(), json!({ "assignee": { "kind": "agent", "id": "codex-cli" }, "due": "2026-12-01" }));
    }

    #[test]
    fn a_views_lists_and_counts_read_as_the_box_says_them() {
        let counts: Counts = serde_json::from_str(r#"{"open":9,"statuses":{"todo":6,"done":0},"priorities":{"urgent":1},"labels":{"lbl_bug":2},"sources":{"github":3},"computers":{"":5,"k1":4},"removed":1}"#).unwrap();
        assert_eq!((counts.open, counts.statuses["todo"], counts.priorities["urgent"], counts.labels["lbl_bug"], counts.sources["github"], counts.computers["k1"], counts.removed), (9, 6, 1, 2, 3, 4, 1));
        // A bot it may be for has its look; one in a shape this can't read is still listed.
        let assignees: Vec<Assignee> = read_all(Some(&json!([{ "kind": "bot", "id": "bot_1", "name": "Mira", "look": { "color": "sky", "shape": "round", "face": "calm" } }, { "kind": "agent", "id": "codex-cli", "name": "Codex", "look": 4 }, { "name": "no id" }])));
        assert_eq!(assignees.len(), 2);
        assert_eq!((assignees[0].look.as_ref().map(|look| look.color.as_str()), assignees[1].look.clone()), (Some("sky"), None));
    }
}
