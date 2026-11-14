//! The Work Queue as a client shows it (PROTOCOL.md, "The Work Queue"): the view across your
//! computers (`kuvo/work-queue?owner=all&counts=1`: its categories, its items in the box's order,
//! who owns them, what's deleted, your other computers and the box's counts), one item whole
//! (`kuvo/work-queue/items/<id>`), and what you do to one: move it (to another category, Done
//! included), delete it, bring it back. Everything is read as the box says it; an item in a shape
//! this can't read is left out, not the list.
//!
//! Adding an item and changing what one says aren't a client's in the protocol yet (agents do
//! both, through their tools): [`BoxClient::add_work_item`] and [`BoxClient::edit_work_item`] send
//! to routes proposed for it, offered only where the box's `features` name [`WRITES`].

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::areas::{Category, QueueCounts};
use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::paths;
use crate::transport::Request;

// BOX-NEEDED: adding an item and editing its title and description from a client
// (`POST kuvo/work-queue/items`, `PATCH kuvo/work-queue/items/<item>`), named in `features`.
/// The feature a box names once a client may add an item and change what one says.
pub const WRITES: &str = "work-queue.edit";

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
    #[serde(default)]
    pub category: String,
    #[serde(default, rename = "inProgress")]
    pub in_progress: bool,
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
    /// Done is someone else's (`github`): never a place to move it to from here.
    #[serde(default, rename = "closesOn")]
    pub closes_on: Option<String>,
    /// What's shown in place of its category while GitHub makes it Done.
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
    /// The categories, in the box's order.
    pub categories: Vec<Category>,
    /// The items, in the box's order.
    pub items: Vec<Item>,
    pub owners: Vec<Owner>,
    /// What's deleted, newest first.
    pub removed: Vec<Item>,
    /// Your other computers that told of their queues.
    pub computers: Vec<Told>,
    /// The box's counts, from this same view.
    pub counts: QueueCounts,
}

/// A new item, as written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewItem {
    pub title: String,
    pub body: String,
    pub category: Option<String>,
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
        let answer: Value = self.get("kuvo/work-queue?owner=all&counts=1")?;
        if !answer.get("items").is_some_and(Value::is_array) {
            return Err(Error::BadAnswer("The box didn’t list its Work Queue.".into()));
        }
        Ok(QueueSaid {
            categories: read_all(answer.get("categories")),
            items: read_all(answer.get("items")),
            owners: read_all(answer.get("owners")),
            removed: read_all(answer.get("removed")),
            computers: read_all(answer.get("computers")),
            counts: answer.get("counts").cloned().and_then(|counts| serde_json::from_value(counts).ok()).unwrap_or_default(),
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

    /// Moves an item to `category` (by its id; `done` marks it done), as the box places it.
    pub fn move_work_item(&self, item: &str, category: &str) -> Result<Item> {
        self.item_answer(Request::post(format!("{}/move", item_route(item)), json!({ "category": category })))
    }

    /// Deletes an item (the box keeps it a while, to bring back).
    pub fn delete_work_item(&self, item: &str) -> Result<Item> {
        self.item_answer(Request::delete(item_route(item)))
    }

    /// Brings a deleted item back.
    pub fn restore_work_item(&self, item: &str) -> Result<Item> {
        self.item_answer(Request::post(format!("{}/restore", item_route(item)), json!({})))
    }

    /// Adds an item (a box that names [`WRITES`]).
    pub fn add_work_item(&self, new: &NewItem) -> Result<Item> {
        let mut body = Map::new();
        body.insert("title".into(), json!(new.title));
        body.insert("body".into(), json!(new.body));
        if let Some(category) = &new.category {
            body.insert("category".into(), json!(category));
        }
        self.item_answer(Request::post("kuvo/work-queue/items", Value::Object(body)))
    }

    /// Changes what an item says: its title and description (a box that names [`WRITES`]).
    pub fn edit_work_item(&self, item: &str, title: &str, body: &str) -> Result<Item> {
        self.item_answer(Request { method: "PATCH".into(), path: item_route(item), headers: Vec::new(), body: Some(json!({ "title": title, "body": body })) })
    }

    /// Makes a category, called `name`.
    pub fn add_category(&self, name: &str) -> Result<Vec<Category>> {
        self.categories_answer(json!({ "add": name }))
    }

    /// Renames a category (by its id).
    pub fn rename_category(&self, category: &str, to: &str) -> Result<Vec<Category>> {
        self.categories_answer(json!({ "rename": category, "to": to }))
    }

    /// Removes a category (by its id), once nothing is in it.
    pub fn remove_category(&self, category: &str) -> Result<Vec<Category>> {
        self.categories_answer(json!({ "remove": category }))
    }

    fn categories_answer(&self, body: Value) -> Result<Vec<Category>> {
        let answer: Value = self.value(&Request::post("kuvo/work-queue/categories", body))?;
        Ok(read_all(answer.get("categories")))
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
            r#"{"id":"wqi_1","number":243,"owner":"ses_a","title":"Fix it","category":"next","order":1,"inProgress":true,"progress":{"value":0.4,"note":"Writing the test"},
            "sessions":[{"session":"ses_a","title":"Fixing","working":true}],"noteCount":2,"fileCount":1,"linkCount":0,
            "created":{"at":5,"by":{"user":true}},"updated":{"at":9,"by":{"session":"ses_a","name":"Mira"}},"newer":{"kept":true}}"#,
        )
        .unwrap();
        assert_eq!(item.label().as_deref(), Some("#243"));
        assert!(item.in_progress && item.created.by.user && item.sessions[0].working);
        assert_eq!((item.updated.by.name.as_deref(), item.progress.as_ref().and_then(|progress| progress.value)), (Some("Mira"), Some(0.4)));

        // An issue is named by GitHub's name for it, never its old number; one waiting for a number has none.
        let issue: Item = serde_json::from_str(r#"{"id":"wqi_2","number":7,"title":"Crash","category":"done","source":{"kind":"github","ref":"gh:acme/site#12","name":"site#12"},"struck":true,"closesOn":"github","doneAs":"Closed on GitHub","created":{"at":1,"by":{"source":"github","name":"octo"}}}"#).unwrap();
        assert_eq!((issue.label().as_deref(), issue.struck, issue.closes_on.as_deref(), issue.created.by.name.as_deref()), (Some("site#12"), true, Some("github"), Some("octo")));
        let waiting: Item = serde_json::from_str(r#"{"id":"wqi_3","title":"New"}"#).unwrap();
        assert_eq!(waiting.label(), None);
    }
}
