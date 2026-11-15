//! A box's settings as it describes them (PROTOCOL.md, "A box's settings", `settings`): its
//! sections, each section's page of rows, a change to one row from the revision it was read at,
//! what a row offers to do, and a page followed as it changes. A client draws each row by its
//! [`Kind`], in the box's words; nothing here knows what any setting is.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::follow::{Follow, Followed, Timing, follow_lines};
use crate::paths::segment;
use crate::transport::Request;

/// One of a box's sections, for a sidebar and search.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    /// The heading it's under ("Agents & Tools").
    #[serde(default)]
    pub group: String,
    /// Its icon by name (`shield`, `globe`): one this client hasn't, it draws with its own.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub line: String,
    /// What search finds it by besides its title and line.
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// A section's page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Page {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub groups: Vec<Group>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub notes: Vec<Note>,
}

/// A card of rows, with its heading and the words under it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Group {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub rows: Vec<Row>,
}

/// Words on a page, after its groups.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Note {
    pub words: String,
    #[serde(default)]
    pub tone: Tone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Warning,
    /// (And one a newer box says that this client doesn't know.)
    #[default]
    #[serde(other)]
    Quiet,
}

/// What a row is drawn as. One a newer box sends that this client doesn't know is `Unknown`: its
/// title and line are drawn, with words that a newer Kuvo Code changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Switch,
    Choice,
    Text,
    Number,
    Folder,
    Secret,
    Status,
    Actions,
    List,
    Meter,
    Open,
    Steps,
    Custom,
    #[serde(other)]
    Unknown,
}

/// Whether a row or an action can be changed from here, or why not, in the box's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Can {
    Yes,
    No(String),
}

impl<'de> Deserialize<'de> for Can {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Can, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Yes(bool),
            No { why: String },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Yes(true) => Can::Yes,
            // (`false` isn't something a box says; read as no, with nothing to show.)
            Wire::Yes(false) => Can::No(String::new()),
            Wire::No { why } => Can::No(why),
        })
    }
}

/// What's asked first, in the box's words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Confirm {
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub button: String,
}

/// Something a row offers to do.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Action {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub confirm: Option<Confirm>,
    /// An address this client opens itself, sending nothing.
    #[serde(default)]
    pub opens: Option<String>,
    /// Absent: it can be done.
    #[serde(default)]
    pub can: Option<Can>,
}

impl Action {
    pub fn is_destructive(&self) -> bool {
        self.style.as_deref() == Some("destructive")
    }

    /// Why it can't be done from here, where it can't.
    pub fn why_not(&self) -> Option<&str> {
        match &self.can {
            Some(Can::No(why)) => Some(why),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Choice {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub line: Option<String>,
}

/// One of a `meter` row's windows.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Window {
    pub title: String,
    /// How much is used, 0 to 1.
    #[serde(default)]
    pub used: f64,
    /// When it starts over (ms since 1970).
    #[serde(default)]
    pub resets: Option<f64>,
}

/// A row, with its kind's fields (those of other kinds are left empty).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default)]
    pub tone: Tone,
    pub kind: Kind,
    #[serde(default)]
    pub value: Option<Value>,
    /// None for a row with nothing to change.
    #[serde(default)]
    pub can: Option<Can>,
    /// Sent back as `was` with a change to it.
    #[serde(default)]
    pub revision: String,
    #[serde(default)]
    pub confirm: Option<Confirm>,
    #[serde(default)]
    pub busy: Option<String>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// A `secret`'s: whether one is set.
    #[serde(default)]
    pub set: bool,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub actions: Vec<Action>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub start: Option<Action>,
    #[serde(default)]
    pub name: Option<String>,
    /// A `list`'s rows.
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub items: Vec<Row>,
    #[serde(default)]
    pub add: Option<Action>,
    #[serde(default)]
    pub empty: Option<String>,
}

impl Row {
    /// Why it can't be changed from here, where it can't.
    pub fn why_not(&self) -> Option<&str> {
        match &self.can {
            Some(Can::No(why)) => Some(why),
            _ => None,
        }
    }

    /// A `switch`'s value.
    pub fn is_on(&self) -> bool {
        self.value.as_ref().and_then(Value::as_bool).unwrap_or(false)
    }

    /// A `choice`'s chosen one, or a `status`'s words.
    pub fn words(&self) -> Option<&str> {
        self.value.as_ref().and_then(Value::as_str)
    }

    /// The choice shown now: the one whose id is the value.
    pub fn chosen(&self) -> Option<&Choice> {
        let value = self.words()?;
        self.choices.iter().find(|choice| choice.id == value)
    }
}

impl Page {
    /// A row by its id, a list's items too.
    pub fn row(&self, id: &str) -> Option<&Row> {
        self.groups.iter().flat_map(|group| &group.rows).flat_map(|row| std::iter::once(row).chain(&row.items)).find(|row| row.id == id)
    }
}

/// What a change or an action came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Changed {
    /// Done: the page as it is now.
    Done(Page),
    /// Refused: the row changed since it was read (`409`). The box's words, and the page as it
    /// is now, to draw instead.
    Stale { message: String, page: Option<Page> },
}

#[derive(Deserialize)]
struct Sections {
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    sections: Vec<Section>,
}

fn page_route(section: &str) -> String {
    format!("kuvo/settings/{}", segment(section))
}

impl BoxClient {
    /// This box's settings sections, in its order.
    pub fn settings_sections(&self) -> Result<Vec<Section>> {
        Ok(self.get::<Sections>("kuvo/settings")?.sections)
    }

    /// A section's page.
    pub fn settings_page(&self, section: &str) -> Result<Page> {
        self.get(&page_route(section))
    }

    /// Follows a section's page: as it is now, and again as it changes.
    pub fn follow_settings_page(&self, section: &str, timing: Timing) -> Follow<Followed<Page>> {
        let route = page_route(section);
        follow_lines(self, "kuvo-box settings", format!("{route}/events"), route, timing)
    }

    /// Changes `row` to `value`, from the revision it was read at.
    pub fn change_setting(&self, section: &str, row: &Row, value: Value) -> Result<Changed> {
        let request = Request { method: "PATCH".into(), path: page_route(section), headers: Vec::new(), body: Some(json!({ "row": row.id, "value": value, "was": row.revision })) };
        changed(self, &request)
    }

    /// Does what `row` offers (`action`, one that doesn't open an address), from the revision it
    /// was read at.
    pub fn setting_action(&self, section: &str, row: &Row, action: &Action) -> Result<Changed> {
        let path = format!("{}/{}/{}", page_route(section), segment(&row.id), segment(&action.id));
        changed(self, &Request::post(path, json!({ "was": row.revision })))
    }
}

fn changed(client: &BoxClient, request: &Request) -> Result<Changed> {
    let response = client.send(request)?;
    if response.status == 409 {
        #[derive(Deserialize)]
        struct Stale {
            #[serde(default)]
            page: Option<Value>,
        }
        let refusal = Refusal::from_answer(response.status, &response.body);
        let page = serde_json::from_slice::<Stale>(&response.body).ok().and_then(|stale| stale.page).and_then(|page| serde_json::from_value(page).ok());
        return Ok(Changed::Stale { message: refusal.message, page });
    }
    if !response.is_success() {
        return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
    }
    serde_json::from_slice(&response.body).map(Changed::Done).map_err(|error| Error::BadAnswer(format!("The answer couldn't be read: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reads_every_kind_and_keeps_a_row_of_a_kind_it_doesnt_know() {
        let page: Page = serde_json::from_value(json!({
            "id": "permissions", "title": "Permissions",
            "groups": [{ "id": "approval", "title": "Approval", "rows": [
                { "id": "full-access", "title": "Full Access", "kind": "switch", "value": false, "can": { "why": "Turn it on in a conversation." }, "revision": "r1" },
                { "id": "other", "title": "Let your other computers start work here", "kind": "switch", "value": true, "can": true, "revision": "r2" },
                { "id": "mode", "title": "New sessions start in", "kind": "status", "value": "Auto", "revision": "r3" },
                { "id": "later", "title": "Something newer", "line": "From a newer box.", "kind": "hologram", "revision": "r4" },
                { "id": "broken", "kind": "switch" }
            ] }, { "id": "allowed", "rows": [
                { "id": "allowed", "title": "Also Allowed", "kind": "list", "revision": "r5", "empty": "Nothing else is allowed yet.", "items": [
                    { "id": "allowed-1", "title": "/srv/cache", "kind": "actions", "revision": "r6", "actions": [{ "id": "remove", "title": "Remove" }] }
                ] }
            ] }],
            "notes": [{ "words": "Rules aren't on this page yet.", "tone": "quiet" }, { "words": "Later", "tone": "glowing" }]
        }))
        .unwrap();
        let ids: Vec<&str> = page.groups[0].rows.iter().map(|row| row.id.as_str()).collect();
        // (A row without a title can't be drawn: left out, the rest kept.)
        assert_eq!(ids, ["full-access", "other", "mode", "later"]);
        assert_eq!(page.row("full-access").unwrap().why_not(), Some("Turn it on in a conversation."));
        assert_eq!(page.row("other").unwrap().can, Some(Can::Yes));
        assert!(page.row("other").unwrap().is_on());
        assert_eq!(page.row("mode").unwrap().words(), Some("Auto"));
        assert_eq!(page.row("later").unwrap().kind, Kind::Unknown);
        assert_eq!(page.row("allowed-1").unwrap().actions[0].title, "Remove");
        assert_eq!(page.notes[1].tone, Tone::Quiet);
    }
}
