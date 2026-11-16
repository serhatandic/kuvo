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
    /// What it asks for before it's done (`settings.flows`).
    #[serde(default)]
    pub form: Option<Form>,
    /// It lets something use what's yours from now on: a client makes sure it's you first.
    #[serde(default)]
    pub verified: bool,
}

/// A form an action asks to be filled in first: its sheet's title, its button's words, its fields
/// in order and a line under them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Form {
    pub title: String,
    pub submit: String,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub note: Option<String>,
    /// Its own revision, sent back as `was` with its values and checks (a box from before forms had
    /// theirs: the row's).
    #[serde(default)]
    pub revision: Option<String>,
}

/// What a form's field is drawn as; one this client doesn't know is `Unknown` (its title is drawn,
/// with words that a newer Kuvo Code changes it, and its value is sent as it came).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    Text,
    Lines,
    Choice,
    Switch,
    Folder,
    Pairs,
    /// Hidden as it's typed (several lines, with `multi`: a note's text, not hidden); it opens empty.
    Secret,
    /// A file chosen on this computer, sent as `{name, data}`.
    File,
    #[serde(other)]
    Unknown,
}

/// One of a form's fields.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Field {
    pub id: String,
    pub title: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub mono: bool,
    #[serde(default)]
    pub folder: Option<Folder>,
    #[serde(default)]
    pub pairs: Option<PairWords>,
    #[serde(default)]
    pub shown: Option<Shown>,
    #[serde(default)]
    pub can: Option<Can>,
    /// A `secret`'s: several lines.
    #[serde(default)]
    pub multi: bool,
    /// A `secret`'s: where the value it keeps is read when you ask to see it.
    #[serde(default)]
    pub shows: Option<Shows>,
    /// A `pairs`' values: hidden as they're typed.
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub file: Option<FileWords>,
    /// It lets something use what's yours from now on: a client makes sure it's you first.
    #[serde(default)]
    pub verified: bool,
}

impl Field {
    /// Whether it's drawn with these values: always, or while the field it follows is one of its own.
    pub fn is_shown(&self, values: &serde_json::Map<String, Value>) -> bool {
        self.shown.as_ref().is_none_or(|shown| values.get(&shown.field).and_then(Value::as_str).is_some_and(|value| shown.is.iter().any(|each| each == value)))
    }

    pub fn why_not(&self) -> Option<&str> {
        match &self.can {
            Some(Can::No(why)) => Some(why),
            _ => None,
        }
    }
}

/// A `pairs` field's placeholders, for a name and its value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PairWords {
    pub key: String,
    pub value: String,
}

/// Where a `secret` field's kept value is read, only when you ask to see it: `GET route` of the
/// same box, whose `values[field]` it is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Shows {
    pub route: String,
    pub field: String,
}

/// A `file` field's words: its button's, and what it says before one is chosen.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileWords {
    pub choose: String,
    pub none: String,
}

/// The most of a chosen file a form sends; one bigger is sent as its name and size alone.
pub const MOST_FILE_BYTES: usize = 1_000_000;

/// A file chosen for a `file` field, as its value: `{name, data}` (its bytes, standard base64), or
/// for one bigger than a form sends (`size`, its real size), `{name, size}`.
pub fn file_value(name: &str, bytes: &[u8], size: u64) -> Value {
    if bytes.len() > MOST_FILE_BYTES || size > MOST_FILE_BYTES as u64 {
        return json!({ "name": name, "size": size });
    }
    use base64::Engine as _;
    json!({ "name": name, "data": base64::engine::general_purpose::STANDARD.encode(bytes) })
}

/// A form's values as a check gets them: what's secret never goes with a check (PROTOCOL.md, "A
/// box's settings"). A `secret` field's text is a `•` for each character, a `secret` pairs' values
/// too (their names as typed), and a `file` is its name and size, without its bytes.
pub fn for_check(fields: &[Field], values: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    fn dots(value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String("\u{2022}".repeat(text.chars().count())),
            // (Anything else in a secret's place is no text to count: sent as nothing.)
            _ => Value::Null,
        }
    }
    let mut checked = values.clone();
    for field in fields {
        let Some(value) = checked.get_mut(&field.id) else { continue };
        match field.kind {
            FieldKind::Secret => *value = dots(value),
            FieldKind::Pairs if field.secret => {
                if let Some(list) = value.as_array_mut() {
                    for pair in list {
                        if let Some(each) = pair.get_mut("value") {
                            *each = dots(each);
                        }
                    }
                }
            }
            FieldKind::File => {
                if let Some(file) = value.as_object() {
                    let size = file.get("size").and_then(Value::as_u64).or_else(|| file.get("data").and_then(Value::as_str).map(base64_size));
                    let mut kept = serde_json::Map::new();
                    if let Some(name) = file.get("name") {
                        kept.insert("name".into(), name.clone());
                    }
                    if let Some(size) = size {
                        kept.insert("size".into(), json!(size));
                    }
                    *value = Value::Object(kept);
                } else if !value.is_null() {
                    *value = Value::Null;
                }
            }
            _ => {}
        }
    }
    checked
}

/// How many bytes standard base64 `data` holds.
fn base64_size(data: &str) -> u64 {
    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count().min(2);
    ((data.len() / 4) * 3).saturating_sub(padding) as u64
}

/// When a field is drawn: while `field` is one of `is`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Shown {
    pub field: String,
    #[serde(default)]
    pub is: Vec<String>,
}

/// How a thing stands, beside its title.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Badge {
    pub words: String,
    #[serde(default)]
    pub tone: BadgeTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BadgeTone {
    Good,
    Busy,
    Warning,
    Failed,
    /// (And one a newer box names.)
    #[default]
    #[serde(other)]
    Quiet,
}

/// One more line under a row, set apart.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Notice {
    pub words: String,
    #[serde(default)]
    pub tone: Tone,
}

/// A flow the box leads for a row (a sign-in in the browser).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Flow {
    pub id: String,
    pub state: FlowState,
    pub title: String,
    #[serde(default)]
    pub line: String,
    /// The page to finish it on: opened by the client that started it, once.
    #[serde(default)]
    pub opens: Option<String>,
    /// Where the page you end on is pasted back, and the action that takes it.
    #[serde(default)]
    pub code: Option<FlowCode>,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowState {
    Waiting,
    Done,
    #[serde(other)]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FlowCode {
    pub title: String,
    pub placeholder: String,
    pub action: String,
}

/// The box's words under one of a form's fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FieldWords {
    pub id: String,
    pub words: String,
    #[serde(default)]
    pub tone: WordsTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WordsTone {
    Good,
    Warning,
    /// (And one a newer box names.)
    #[default]
    #[serde(other)]
    Quiet,
}

/// A form's values checked as they're typed: each field's words, whether it can be sent, and
/// values the box fills in.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Checked {
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub fields: Vec<FieldWords>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub values: Option<serde_json::Map<String, Value>>,
}

/// What a form sent came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// Done, or stale: as a change.
    Changed(Changed),
    /// Its values refused: why, and each field's words (the form stays).
    Refused { message: String, fields: Vec<FieldWords> },
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Lines { pub min: usize, pub max: usize }
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Folder { pub choose: String, pub clear: String, pub prompt: String, pub message: String }
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Search { pub placeholder: String, pub empty: String }
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Suggestion { pub value: f64, pub title: String }
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ConfirmBelow { pub value: f64, pub confirm: Confirm }

/// One part of a `meter` row's whole (what Kuvo keeps on a computer): its name, how much, its words,
/// and what it's made of.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Part {
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub value: f64,
    #[serde(default)]
    pub words: String,
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Unit { pub name: String, pub factor: f64, pub decimals: usize, pub rounding: Rounding }
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Rounding { Nearest }

impl Unit {
    pub fn number(&self, value: f64) -> String {
        let scale = 10_f64.powi(self.decimals as i32);
        let number = match self.rounding { Rounding::Nearest => (value / self.factor * scale).round() / scale };
        let number = if number == 0.0 { 0.0 } else { number };
        let text = format!("{:.*}", self.decimals, number);
        if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text }
    }
    pub fn display(&self, value: f64) -> String { format!("{} {}", self.number(value), self.name) }
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
    pub lines: Option<Lines>,
    #[serde(default)]
    pub folder: Option<Folder>,
    #[serde(default)]
    pub search: Option<Search>,
    #[serde(default)]
    pub unit: Option<Unit>,
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    #[serde(default, rename = "confirmBelow")]
    pub confirm_below: Option<ConfirmBelow>,
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
    /// A `meter`'s parts of a whole, instead of windows.
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub parts: Vec<Part>,
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
    /// How it stands, beside its title (`settings.flows`).
    #[serde(default)]
    pub badge: Option<Badge>,
    #[serde(default)]
    pub notice: Option<Notice>,
    #[serde(default)]
    pub flow: Option<Flow>,
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
    /// Refused: the row changed since it was read (`409` with the page). The box's words, and the
    /// page as it is now, to draw instead.
    Stale { message: String, page: Page },
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
        self.change_setting_at(section, &row.id, &row.revision, value)
    }

    /// Changes the row `row` to `value`, from the revision (`was`) it was read at.
    pub fn change_setting_at(&self, section: &str, row: &str, was: &str, value: Value) -> Result<Changed> {
        self.change_setting_confirmed_at(section, row, was, value, false)
    }

    pub fn change_setting_confirmed_at(&self, section: &str, row: &str, was: &str, value: Value, confirmed: bool) -> Result<Changed> {
        let mut body = json!({ "row": row, "value": value, "was": was });
        if confirmed { body["confirmed"] = json!(true); }
        let request = Request { method: "PATCH".into(), path: page_route(section), headers: Vec::new(), body: Some(body) };
        changed(self, &request)
    }

    /// Does what `row` offers (`action`, one that doesn't open an address), from the revision it
    /// was read at.
    pub fn setting_action(&self, section: &str, row: &Row, action: &Action) -> Result<Changed> {
        self.setting_action_at(section, &row.id, &row.revision, &action.id)
    }

    /// Does what the row `row` offers (`action`), from the revision (`was`) it was read at.
    pub fn setting_action_at(&self, section: &str, row: &str, was: &str, action: &str) -> Result<Changed> {
        let path = format!("{}/{}/{}", page_route(section), segment(row), segment(action));
        changed(self, &Request::post(path, json!({ "was": was })))
    }
}

impl BoxClient {
    /// Does what the row `row` offers (`action`) with its form's values (or a flow's code), from
    /// the revision (`was`) it was read at.
    pub fn setting_form_at(&self, section: &str, row: &str, was: &str, action: &str, values: &serde_json::Map<String, Value>) -> Result<Sent> {
        let path = format!("{}/{}/{}", page_route(section), segment(row), segment(action));
        let request = Request::post(path, json!({ "was": was, "values": values }));
        let response = self.send(&request)?;
        if response.status == 400 {
            #[derive(Deserialize)]
            struct Refused {
                #[serde(default)]
                message: String,
                #[serde(default, deserialize_with = "crate::accounts::readable")]
                fields: Vec<FieldWords>,
            }
            if let Ok(refused) = serde_json::from_slice::<Refused>(&response.body) {
                return Ok(Sent::Refused { message: refused.message, fields: refused.fields });
            }
        }
        answered(response).map(Sent::Changed)
    }

    /// The value a `secret` field keeps, read only when you ask to see it (`shows`): `GET route`'s
    /// `values[field]`, the rest of the answer dropped as it's read. (Never in an error's words.)
    pub fn shown_secret(&self, shows: &Shows) -> Result<String> {
        let mut answer: Value = self.value(&Request::get(shows.route.trim_start_matches('/')))?;
        let value = answer.get_mut("values").and_then(|values| values.get_mut(&shows.field)).map(Value::take);
        drop(answer);
        match value {
            Some(Value::String(text)) => Ok(text),
            _ => Err(Error::BadAnswer("There's nothing to show.".into())),
        }
    }

    /// A form's values checked as they're typed, changing nothing.
    pub fn check_setting_form(&self, section: &str, row: &str, was: &str, action: &str, values: &serde_json::Map<String, Value>) -> Result<Checked> {
        let path = format!("{}/{}/{}/check", page_route(section), segment(row), segment(action));
        let response = self.send(&Request::post(path, json!({ "was": was, "values": values })))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        serde_json::from_slice(&response.body).map_err(|error| Error::BadAnswer(format!("The answer couldn't be read: {error}")))
    }
}

fn changed(client: &BoxClient, request: &Request) -> Result<Changed> {
    answered(client.send(request)?)
}

fn answered(response: crate::transport::Response) -> Result<Changed> {
    // (A 409 without the page is the route underneath refusing, a settings file that can't be
    // used: a refusal in its words, like any other.)
    if response.status == 409 {
        #[derive(Deserialize)]
        struct Stale {
            page: Page,
        }
        if let Ok(stale) = serde_json::from_slice::<Stale>(&response.body) {
            return Ok(Changed::Stale { message: Refusal::from_answer(response.status, &response.body).message, page: stale.page });
        }
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

#[cfg(test)]
mod row_metadata_tests {
    use super::*;
    #[test]
    fn reads_every_presentation_and_confirmation_field() {
        let row: Row = serde_json::from_value(json!({
            "id":"budget", "title":"Budget", "kind":"number", "value":2000000000,
            "unit":{"name":"GB","factor":1000000000,"decimals":2,"rounding":"nearest"}, "suggestions":[{"value":1000000000,"title":"1 GB"}],
            "confirmBelow":{"value":1500000000,"confirm":{"title":"Keep less?","body":"Old copies go.","button":"Keep"}},
            "folder":{"choose":"Choose…","clear":"Default","prompt":"Choose","message":"Choose a folder."},
            "lines":{"min":3,"max":8},"search":{"placeholder":"Find","empty":"Nothing for {query}"}
        })).unwrap();
        assert_eq!(row.unit.as_ref().unwrap().name,"GB");
        assert_eq!(row.suggestions[0].title,"1 GB");
        assert_eq!(row.confirm_below.unwrap().confirm.body,"Old copies go.");
        assert_eq!(row.folder.unwrap().prompt,"Choose");
        assert_eq!(row.lines.unwrap().min,3);
        assert_eq!(row.search.unwrap().empty,"Nothing for {query}");
    }

    #[test]
    fn formats_units_with_nearest_rounding_and_trimmed_trailing_zeroes() {
        let gb = Unit { name: "GB".into(), factor: 1_000_000_000.0, decimals: 2, rounding: Rounding::Nearest };
        assert_eq!(gb.number(1073741824.0), "1.07");
        assert_eq!(gb.display(500000000.0), "0.5 GB");
        assert_eq!(gb.display(250000000.0), "0.25 GB");
        assert_eq!(gb.number(-0.0), "0");
        let tenths = Unit { name: "GB".into(), factor: 1_000_000_000.0, decimals: 1, rounding: Rounding::Nearest };
        assert_eq!(tenths.display(1342177280.0), "1.3 GB");
    }
}

#[cfg(test)]
mod form_field_tests {
    use super::*;

    fn form() -> Form {
        serde_json::from_value(json!({ "title": "Edit", "submit": "Save", "fields": [
            { "id": "password", "title": "Password", "kind": "secret", "placeholder": "Unchanged", "shows": { "route": "/kuvo/things/1/shown", "field": "password" } },
            { "id": "note", "title": "Note", "kind": "secret", "multi": true },
            { "id": "env", "title": "Values", "kind": "pairs", "secret": true, "pairs": { "key": "NAME", "value": "value" } },
            { "id": "headers", "title": "Headers", "kind": "pairs", "pairs": { "key": "Header", "value": "value" } },
            { "id": "key", "title": "Key file", "kind": "file", "file": { "choose": "Choose…", "none": "No file" } },
            { "id": "name", "title": "Name", "kind": "text" },
            { "id": "later", "title": "Later", "kind": "hologram" },
            { "id": "who", "title": "Who can use it", "kind": "choice", "verified": true, "choices": [] }
        ] }))
        .unwrap()
    }

    #[test]
    fn a_form_reads_secrets_files_and_what_an_older_box_leaves_out() {
        let form = form();
        let kinds: Vec<FieldKind> = form.fields.iter().map(|field| field.kind).collect();
        assert_eq!(kinds, [FieldKind::Secret, FieldKind::Secret, FieldKind::Pairs, FieldKind::Pairs, FieldKind::File, FieldKind::Text, FieldKind::Unknown, FieldKind::Choice]);
        assert!(form.fields[7].verified && !form.fields[0].verified);
        assert_eq!(form.fields[0].shows, Some(Shows { route: "/kuvo/things/1/shown".into(), field: "password".into() }));
        assert!(!form.fields[0].multi && form.fields[1].multi);
        assert!(form.fields[2].secret && !form.fields[3].secret);
        assert_eq!(form.fields[4].file, Some(FileWords { choose: "Choose…".into(), none: "No file".into() }));
        // (A box from before these fields: none of them, and the field reads as it did.)
        let name = &form.fields[5];
        assert!(!name.multi && !name.secret && !name.verified && name.shows.is_none() && name.file.is_none());
        let actions: Vec<Action> = serde_json::from_value(json!([{ "id": "grant", "title": "Let It Use…", "verified": true }, { "id": "remove", "title": "Remove" }])).unwrap();
        assert!(actions[0].verified && !actions[1].verified);
    }

    #[test]
    fn a_check_gets_dots_for_secrets_and_a_files_name_and_size_and_the_values_stay_as_typed() {
        let form = form();
        let values = json!({
            "password": "hunter2é",
            "note": "line one\nline two",
            "env": [{ "key": "TOKEN", "value": "abc123" }, { "key": "EMPTY", "value": "" }],
            "headers": [{ "key": "Accept", "value": "text/plain" }],
            "key": file_value("id_ed25519", b"secret bytes", 12),
            "name": "Mine",
            "later": { "kept": true }
        });
        let values = values.as_object().unwrap().clone();
        let checked = for_check(&form.fields, &values);
        assert_eq!(checked["password"], json!("••••••••"));
        assert_eq!(checked["note"], json!("•".repeat(17)));
        assert_eq!(checked["env"], json!([{ "key": "TOKEN", "value": "••••••" }, { "key": "EMPTY", "value": "" }]));
        assert_eq!(checked["headers"], values["headers"]);
        assert_eq!(checked["key"], json!({ "name": "id_ed25519", "size": 12 }));
        assert_eq!((checked["name"].clone(), checked["later"].clone()), (json!("Mine"), json!({ "kept": true })));
        // What's sent with the action itself is what was typed.
        assert_eq!(values["key"], json!({ "name": "id_ed25519", "data": "c2VjcmV0IGJ5dGVz" }));
        assert!(!serde_json::to_string(&checked).unwrap().contains("hunter2") && !serde_json::to_string(&checked).unwrap().contains("abc123"));
    }

    #[test]
    fn a_files_size_is_counted_from_its_data_and_one_too_big_goes_without_it() {
        for size in 0..7usize {
            let bytes = vec![7u8; size];
            let value = file_value("f", &bytes, size as u64);
            let checked = for_check(&form().fields, json!({ "key": value }).as_object().unwrap());
            assert_eq!(checked["key"]["size"], json!(size), "{size} bytes");
        }
        let big = file_value("big.bin", &vec![0u8; MOST_FILE_BYTES + 1], 5_000_000);
        assert_eq!(big, json!({ "name": "big.bin", "size": 5_000_000 }));
        // As the form opened (a file kept before, named): its name, as it was.
        let kept = for_check(&form().fields, json!({ "key": { "name": "old.pem" } }).as_object().unwrap());
        assert_eq!(kept["key"], json!({ "name": "old.pem" }));
        let none = for_check(&form().fields, json!({ "key": null }).as_object().unwrap());
        assert_eq!(none["key"], Value::Null);
    }
}
