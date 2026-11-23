//! The Marketplace (`kuvo/marketplace`, PROTOCOL.md "Marketplace"): apps, ready-made agents and
//! skills, free, for the whole setup. The box serves the catalog, its categories and counts, each
//! item's state and the words of every action; a client draws what it says and sends what you do.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::Result;
use crate::bots::Made;
use crate::models::Offered;
use crate::typed::ModelRef;
use crate::paths::segment;
use crate::transport::Request;
use crate::BoxClient;

const ROOT: &str = "kuvo/marketplace";

/// What the Marketplace shows now: its computers and its three areas (Apps, Agents, Skills).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Market {
    pub title: String,
    #[serde(default)]
    pub computers: Vec<MarketComputer>,
    #[serde(default, rename = "searchPlaceholder")]
    pub search_placeholder: String,
    #[serde(default)]
    pub areas: Vec<MarketArea>,
}

impl Market {
    pub fn area(&self, id: &str) -> Option<&MarketArea> {
        self.areas.iter().find(|area| area.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MarketComputer {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub here: bool,
    #[serde(default)]
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Category {
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct MarketArea {
    pub id: String,
    pub title: String,
    /// The quiet line under its title.
    #[serde(default)]
    pub line: String,
    pub count: usize,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub items: Vec<MarketItem>,
    #[serde(default)]
    pub actions: Vec<MarketAction>,
    #[serde(default)]
    pub filters: Vec<MarketAction>,
    #[serde(default)]
    pub empty: String,
    /// Skills: this computer's own (each skill's switch, Recently Deleted, Other Places).
    #[serde(default)]
    pub here: Option<SkillsHere>,
}

impl MarketArea {
    pub fn action(&self, id: &str) -> Option<&MarketAction> {
        self.actions.iter().find(|action| action.id == id)
    }
}

/// Words with a tone (`good`, `warning`), shown as they are.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub words: String,
    #[serde(default)]
    pub tone: String,
}

impl Status {
    pub fn good(&self) -> bool {
        self.tone == "good"
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Routine {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub every: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub enabled: bool,
}

/// One app, agent or skill as a list shows it: what's common, and each kind's own.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct MarketItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub installed: bool,
    /// An app's logo, a route answering PNG.
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub connection: Option<String>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub actions: Vec<MarketAction>,
    /// An agent's words under its name (what it needs, its bots added).
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub routines: Vec<Routine>,
    /// A skill on this computer: whether it's on, and why it can't be switched where it can't.
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, rename = "switchWhy")]
    pub switch_why: Option<String>,
    /// A skill found in another folder: where (a path, or "Built in").
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub source: Option<Value>,
}

impl MarketItem {
    pub fn is_yours(&self) -> bool {
        self.origin.as_deref().is_some_and(|origin| origin != "catalog")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct MarketAction {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub form: Option<MarketForm>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// `destructive` for what removes something.
    #[serde(default)]
    pub style: Option<String>,
    /// Asked first, in these words.
    #[serde(default)]
    pub confirm: Option<Confirm>,
}

impl MarketAction {
    pub fn destructive(&self) -> bool {
        self.style.as_deref() == Some("destructive")
    }
}

/// A confirm in the box's words: its question, what it comes to, and the button that goes ahead.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub button: String,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct MarketForm {
    pub title: String,
    pub submit: String,
    #[serde(default)]
    pub fields: Vec<FormField>,
    /// Where to get what's asked (a token's page).
    #[serde(default)]
    pub help: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct FormField {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub placeholder: Option<String>,
    /// `lines` (several lines), `password`; none: one line of text.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub mono: bool,
    /// What's there now.
    #[serde(default)]
    pub value: Option<String>,
    /// The words under it.
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default)]
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Choice {
    pub id: String,
    pub title: String,
}

/// This computer's skills, as Settings › Skills had them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct SkillsHere {
    #[serde(default)]
    pub problem: Option<String>,
    #[serde(default)]
    pub computer: String,
    #[serde(default, rename = "switchTitle")]
    pub switch_title: String,
    #[serde(default)]
    pub deleted: Option<Deleted>,
    #[serde(default)]
    pub places: Option<Places>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Deleted {
    pub title: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub items: Vec<MarketItem>,
    pub action: MarketAction,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Places {
    pub title: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub items: Vec<Place>,
    pub add: MarketAction,
    pub remove: MarketAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Place {
    pub id: String,
    pub title: String,
}

/// An app's details: its card, and what connecting asks.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AppDetails {
    #[serde(flatten)]
    pub item: MarketItem,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub form: Option<MarketForm>,
}

/// What Connect answered: connected (or still to sign in), or a sign-in to open in the browser.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Connected {
    SignIn(SignIn),
    Done {
        #[serde(default)]
        connection: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SignIn {
    pub attempt: String,
    pub opens: String,
    pub title: String,
    #[serde(rename = "callbackLabel")]
    pub callback_label: String,
    pub submit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SignInState {
    pub state: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AgentBot {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub computer: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AgentLabels {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub computer: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub apps: Option<String>,
    #[serde(default)]
    pub routines: Option<String>,
    #[serde(default)]
    pub skills: Option<String>,
    #[serde(default)]
    pub off: Option<String>,
}

/// An agent's details: its card, the bots made from it, the computers it can run on and the Add
/// sheet's words.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AgentDetails {
    #[serde(flatten)]
    pub item: MarketItem,
    #[serde(default)]
    pub bots: Vec<AgentBot>,
    #[serde(default)]
    pub computers: Vec<MarketComputer>,
    #[serde(default)]
    pub labels: AgentLabels,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MissingApp {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub connect: bool,
    #[serde(default)]
    pub words: String,
}

/// What Add offers on a computer: its models and accounts, the apps still to connect there and
/// the agent's routines (each off).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AgentOptions {
    #[serde(flatten)]
    pub offered: Offered,
    #[serde(default)]
    pub missing: Vec<MissingApp>,
    #[serde(default)]
    pub routines: Vec<Routine>,
    #[serde(default)]
    pub blocked: Option<String>,
}

/// An agent added: on a computer, with a model, its routines turned on, under a name.
#[derive(Clone, Debug, PartialEq)]
pub struct AddAgent {
    pub computer: String,
    pub model: ModelRef,
    pub version: String,
    pub name: String,
    pub routines: Vec<String>,
    /// The same for a retry, so the bot is made once.
    pub request: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub data: String,
    #[serde(default)]
    pub sha256: String,
}

impl SkillFile {
    /// Its text, where it is text.
    pub fn text(&self) -> Option<String> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&self.data).ok()?;
        String::from_utf8(bytes).ok()
    }
}

/// A skill's preview: what's in it, exactly as it will be installed.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SkillPreview {
    pub preview: String,
    pub skill: PreviewSkill,
    #[serde(default)]
    pub files: Vec<SkillFile>,
    pub action: MarketAction,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct PreviewSkill {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: Option<SkillSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SkillSource {
    pub repository: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub commit: String,
}

/// A skill's details: its card, its files, what's installed and what's available.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct SkillDetails {
    #[serde(flatten)]
    pub item: MarketItem,
    #[serde(default)]
    pub files: Vec<SkillFile>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub available: Option<Available>,
    #[serde(default)]
    pub description: Option<String>,
    /// A local skill's whole text (`kuvo/skills/<id>`).
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Available {
    pub version: String,
    pub revision: String,
}

/// What the Marketplace is asked for: search words, a category, and Kuvo's or yours.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketQuery {
    pub words: String,
    pub category: Option<String>,
    pub origin: Option<String>,
}

impl MarketQuery {
    fn route(&self) -> String {
        let mut parts = Vec::new();
        let words = self.words.trim();
        if !words.is_empty() {
            parts.push(format!("q={}", segment(words)));
        }
        if let Some(category) = &self.category {
            parts.push(format!("category={}", segment(category)));
        }
        if let Some(origin) = &self.origin {
            parts.push(format!("origin={}", segment(origin)));
        }
        if parts.is_empty() { ROOT.to_owned() } else { format!("{ROOT}?{}", parts.join("&")) }
    }
}

impl BoxClient {
    /// The Marketplace as it is now, narrowed as asked (the box decides what matches).
    pub fn marketplace(&self, query: &MarketQuery) -> Result<Market> {
        self.get(&query.route())
    }

    /// An app's logo (PNG).
    pub fn marketplace_logo(&self, route: &str) -> Result<Vec<u8>> {
        let response = self.send(&Request::get(route))?;
        if response.is_success() {
            Ok(response.body)
        } else {
            Err(crate::Error::Refused(crate::Refusal::from_answer(response.status, &response.body)))
        }
    }

    pub fn marketplace_app(&self, id: &str) -> Result<AppDetails> {
        self.get(&format!("{ROOT}/apps/{}", segment(id)))
    }

    pub fn add_marketplace_app(&self, id: &str) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/apps/{}/add", segment(id)), json!({})))
    }

    /// Adds your own MCP server, for every computer: `values` as its form has them.
    pub fn add_custom_app(&self, values: &serde_json::Map<String, Value>) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/apps/custom"), Value::Object(values.clone())))
    }

    pub fn connect_marketplace_app(&self, id: &str, token: Option<&str>) -> Result<Connected> {
        let body = token.map_or_else(|| json!({}), |token| json!({ "token": token }));
        self.value(&Request::post(format!("{ROOT}/apps/{}/connect", segment(id)), body))
    }

    pub fn marketplace_sign_in(&self, id: &str, attempt: &str) -> Result<SignInState> {
        self.get(&format!("{ROOT}/apps/{}/status?attempt={}", segment(id), segment(attempt)))
    }

    /// Finishes a sign-in from another computer's browser with the address it ended on.
    pub fn finish_marketplace_sign_in(&self, id: &str, attempt: &str, address: &str) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/apps/{}/finish", segment(id)), json!({ "attempt": attempt, "address": address })))
    }

    pub fn cancel_marketplace_sign_in(&self, id: &str, attempt: &str) -> Result<()> {
        self.done(&Request::post(format!("{ROOT}/apps/{}/cancel", segment(id)), json!({ "attempt": attempt })))
    }

    pub fn disconnect_marketplace_app(&self, id: &str) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/apps/{}/disconnect", segment(id)), json!({})))
    }

    pub fn remove_marketplace_app(&self, id: &str) -> Result<Value> {
        self.value(&Request::delete(format!("{ROOT}/apps/{}", segment(id))))
    }

    pub fn marketplace_agent(&self, id: &str) -> Result<AgentDetails> {
        self.get(&format!("{ROOT}/agents/{}", segment(id)))
    }

    pub fn agent_options(&self, id: &str, computer: &str) -> Result<AgentOptions> {
        self.get(&format!("{ROOT}/agents/{}/options?computer={}", segment(id), segment(computer)))
    }

    pub fn add_marketplace_agent(&self, id: &str, add: &AddAgent) -> Result<Made> {
        let model = json!({ "providerID": add.model.provider_id, "id": add.model.id });
        let body = json!({ "computer": add.computer, "model": model, "version": add.version, "request": add.request, "name": add.name, "routines": add.routines });
        self.value(&Request::post(format!("{ROOT}/agents/{}/add", segment(id)), body))
    }

    /// A skill's preview from what was typed or pasted: a GitHub link, or a folder (the box tells
    /// which).
    pub fn preview_skill(&self, from: &str) -> Result<SkillPreview> {
        self.value(&Request::post(format!("{ROOT}/skills/preview"), json!({ "from": from })))
    }

    /// An installed GitHub skill's newest version, previewed.
    pub fn preview_skill_update(&self, id: &str) -> Result<SkillPreview> {
        self.value(&Request::post(format!("{ROOT}/skills/{}/update-preview", segment(id)), json!({})))
    }

    /// Installs exactly the bytes previewed (`was`: the revision an update replaces).
    pub fn install_skill(&self, preview: &str, was: Option<&str>) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/skills/install"), json!({ "preview": preview, "was": was })))
    }

    pub fn marketplace_skill(&self, id: &str) -> Result<SkillDetails> {
        self.get(&format!("{ROOT}/skills/{}", segment(id)))
    }

    /// Installs a catalog skill at the version and revision shown.
    pub fn add_catalog_skill(&self, id: &str, available: &Available, was: Option<&str>) -> Result<Value> {
        let body = json!({ "version": available.version, "revision": available.revision, "was": was });
        self.value(&Request::post(format!("{ROOT}/skills/{}/add", segment(id)), body))
    }

    pub fn create_skill(&self, name: &str, description: &str, instructions: &str) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/skills/create"), json!({ "name": name, "description": description, "instructions": instructions })))
    }

    pub fn edit_skill(&self, id: &str, description: &str, instructions: &str, was: Option<&str>) -> Result<Value> {
        let request = Request {
            method: "PATCH".into(),
            path: format!("{ROOT}/skills/{}", segment(id)),
            headers: Vec::new(),
            body: Some(json!({ "description": description, "instructions": instructions, "was": was })),
        };
        self.value(&request)
    }

    pub fn remove_skill(&self, id: &str, was: Option<&str>) -> Result<Value> {
        let mut request = Request::delete(format!("{ROOT}/skills/{}", segment(id)));
        request.body = Some(json!({ "was": was }));
        self.value(&request)
    }

    /// Turns a skill on or off on this computer.
    pub fn switch_skill(&self, id: &str, on: bool) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/skills/{}/enabled", segment(id)), json!({ "on": on })))
    }

    pub fn restore_skill(&self, id: &str) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/skills/restore"), json!({ "id": id })))
    }

    pub fn add_skill_place(&self, place: &str) -> Result<Value> {
        self.value(&Request::post(format!("{ROOT}/skills/places"), json!({ "place": place })))
    }

    pub fn remove_skill_place(&self, index: usize) -> Result<Value> {
        let mut request = Request::delete(format!("{ROOT}/skills/places"));
        request.body = Some(json!({ "index": index }));
        self.value(&request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connect_answer_reads_as_a_sign_in_or_as_done() {
        let sign_in: Connected = serde_json::from_value(json!({ "attempt": "a1", "opens": "https://x.test/auth", "title": "Sign in to Linear", "callbackLabel": "Paste", "submit": "Finish Sign-in" })).unwrap();
        assert!(matches!(sign_in, Connected::SignIn(SignIn { ref attempt, .. }) if attempt == "a1"));
        let done: Connected = serde_json::from_value(json!({ "connection": "ready", "problems": [] })).unwrap();
        assert_eq!(done, Connected::Done { connection: Some("ready".into()) });
    }
}
