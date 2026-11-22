//! A conversation's model, as its box says (PROTOCOL.md, "A session's model", `sessions.models`),
//! the models a picker offers ("The models offered", `models.offered`), a model's efforts (its
//! variants, as the engine or the coding agents' bridge lists them, `api/model`), picking one for a
//! conversation (`api/session/<id>/model`), and starting a conversation (`kuvo/sessions`,
//! `sessions.start.empty`).
//!
//! The box decides what runs, which model a new one starts on, and what a model that can't run
//! means for a composer, in its own words (`refusal`, `choose`, `action`); a client draws the chip,
//! the picker and why Send waits from what it says.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::json;

use crate::client::{BoxClient, decode};
use crate::error::{Error, Refusal, Result};
use crate::messages::Where;
use crate::paths;
use crate::transport::Request;
use crate::typed::ModelRef;

/// An account a model needs, as the box names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// A plan whose models are under a coding agent ("SuperGrok", under Grok).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Plan {
    pub name: String,
    pub agent: String,
    pub provider: String,
}

/// What to offer doing about a model that can't run, as its box words it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Action {
    /// `signIn`, `addKey`, `install` (its account's) or `choose` (a model).
    #[serde(rename = "do")]
    pub kind: String,
    /// A button's words ("Sign In", "Choose a Model").
    pub label: String,
    /// What it does ("Sign in to Anthropic").
    pub help: String,
}

/// What a session's box says of its model, and what that means for a composer, decided and worded
/// by the box (`models.readiness`): a client draws `refusal`, `choose` and `action` as they are.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelState {
    /// The model it runs on (its own, else the engine's default); none when there's none to run on.
    pub model: Option<ModelRef>,
    /// As its provider or coding agent lists it.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether it can run here now.
    pub runs: bool,
    /// What it needs, when it can't: `model`, `signIn`, `key`, `install`, `gone`.
    #[serde(default)]
    pub missing: Option<String>,
    #[serde(default)]
    pub account: Option<Account>,
    #[serde(default)]
    pub plan: Option<Plan>,
    /// Why a message on it can't go, and what to do, in the box's words; none where one can go.
    #[serde(default)]
    pub refusal: Option<String>,
    /// The chip says "Choose a Model" and Send waits for one (a model gone, or none to run on).
    #[serde(default)]
    pub choose: bool,
    /// What to offer doing about it (none for one a newer box words otherwise: the rest stands).
    #[serde(default, deserialize_with = "lenient")]
    pub action: Option<Action>,
    /// Its agent picks it (a coding agent's subagent): the chip names it and offers no choice.
    #[serde(default)]
    pub fixed: bool,
}

/// An action as the box says it; one this client can't read is none, never the whole answer lost.
pub(crate) fn lenient<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<Action>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

impl ModelState {
    /// As without the box's word on it (a model just picked, or one it can't say about now): it
    /// runs, as far as this client says, with no refusal of its own.
    pub fn unsaid(&mut self) {
        self.runs = true;
        self.missing = None;
        self.account = None;
        self.plan = None;
        self.refusal = None;
        self.choose = false;
        self.action = None;
    }
}

/// Where a conversation on a model runs: the box's engine, or its coding agents' bridge (Custom
/// Auto's lists too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Engine,
    Agents,
}

impl Side {
    /// The side a conversation in `place` is on.
    pub fn of(place: Where) -> Side {
        match place {
            Where::Engine => Side::Engine,
            Where::Agents => Side::Agents,
        }
    }
}

/// A model a picker offers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OfferedModel {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
    pub name: String,
    /// Where a conversation on it runs: a conversation is put on one of its own side's in place,
    /// and goes to another side's as a new session (`models.offered.sides`, which the gate needs).
    pub side: Side,
    /// Its efforts (variants), as its side lists them.
    pub efforts: Vec<String>,
    /// Other ids a conversation may name it by (Claude Code's `default`).
    pub aliases: Vec<String>,
    /// Its provider is retiring it: a picker leaves it out.
    #[serde(default)]
    pub retiring: bool,
    /// Its agent keeps it out of its picker: a picker leaves it out.
    #[serde(default)]
    pub hidden: bool,
    /// It needs no account (OpenCode Zen's free models, a local one's), as the box says
    /// (`models.offered.free`): left out, it needs one.
    #[serde(default)]
    pub free: bool,
}

impl OfferedModel {
    pub fn reference(&self) -> ModelRef {
        ModelRef { provider_id: self.provider_id.clone(), id: self.id.clone(), variant: None }
    }

    /// Whether a picker shows it.
    pub fn is_offered(&self) -> bool {
        !self.retiring && !self.hidden
    }

    /// Whether it's `model`: the same provider, and its id or one it's known by.
    pub fn is(&self, model: &ModelRef) -> bool {
        self.provider_id == model.provider_id && (self.id == model.id || self.aliases.contains(&model.id))
    }
}

/// Whether each side of the box could say its models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct Known {
    #[serde(default)]
    pub engine: bool,
    #[serde(default)]
    pub agents: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unavailable {
    pub id: String,

}

/// A provider of the models offered, named as its side names it (`models.offered.providers`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// Its account's own label (a coding agent's second sign-in, "Work").
    #[serde(default)]
    pub label: Option<String>,
    /// Which of your accounts with a coding agent it is (`models.offered.accounts`).
    #[serde(default)]
    pub account: Option<ProviderAccount>,
}

/// A coding agent's account, as the box names it for a picker (`models.offered.accounts`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccount {
    /// Its agent (`claude`).
    pub agent: String,
    /// `default` for the agent's own sign-in, else the one you added's.
    pub id: String,
    /// Its agent's name ("Claude Code").
    pub agent_name: String,
    /// The account's name to show ("Work", "sam@example.com").
    pub name: String,
    /// Its plan ("Max"), where the box has read it.
    #[serde(default)]
    pub plan: Option<String>,
}

/// A starred model: its provider and id (`models.starred`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
pub struct Starred {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
}

/// The models offered, in the order a picker draws them, and the one a new session starts on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Offered {
    #[serde(default)]
    pub scoped: bool,
    #[serde(default)]
    pub note: Option<String>,
    pub models: Vec<OfferedModel>,
    /// Each provider's name (none from a box before `models.offered.providers`).
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// Your starred models, in the order starred, as the box keeps them for every client; `None`
    /// from a box that keeps none (before `models.starred`): stars can't be kept there.
    #[serde(default)]
    pub starred: Option<Vec<Starred>>,
    #[serde(default)]
    pub known: Known,
    /// The coding agents' accounts whose list isn't all they run yet: one of theirs not listed
    /// may still run.
    #[serde(default)]
    pub listing: Vec<String>,
    #[serde(default)]
    pub unavailable: Vec<Unavailable>,
    #[serde(default)]
    pub generations: std::collections::HashMap<String, String>,
    /// The model a new session starts on (`None` left out: the box couldn't say; `Some(None)`:
    /// nothing runs there).
    #[serde(default, deserialize_with = "present")]
    pub starting: Option<Option<ModelRef>>,
    #[serde(default, rename = "startChoice")]
    pub start_choice: Option<StartChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StartChoice {
    pub label: String,
    pub refusal: String,
}

pub const MODEL_START_CHOICE: &str = "models.offered.choice";
pub const MODEL_DEFAULTS: &str = "models.offered.defaults";

impl Offered {
    /// `provider`'s name, as its side names it (its id where the box names none).
    pub fn provider_name(&self, provider: &str) -> String {
        self.providers.iter().find(|each| each.id == provider).map(|each| each.name.clone()).unwrap_or_else(|| provider.to_owned())
    }

    /// Whether both sides could say their models.
    pub fn is_whole(&self) -> bool {
        self.known.engine && self.known.agents && self.unavailable.is_empty()
    }

    /// This answer, with what `before` said of a side this one couldn't (PROTOCOL.md, "The models
    /// offered": a client keeps what the box said of that side before, and asks again in a moment),
    /// as KuvoClient's `OfferedModels.keeping`: the engine's ahead, the agents' (and their
    /// `listing`) after.
    pub fn keeping(mut self, before: Option<&Offered>) -> Offered {
        let Some(before) = before else { return self };
        let on = |side: Side| move |model: &&OfferedModel| model.side == side;
        if !self.known.engine && before.known.engine {
            let kept: Vec<OfferedModel> = before.models.iter().filter(on(Side::Engine)).cloned().collect();
            self.models.splice(0..0, kept);
            self.known.engine = true;
        }
        for each in &self.unavailable {
            let same_account = self.generations.get(&each.id).is_some_and(|generation| before.generations.get(&each.id) == Some(generation));
            if same_account && !self.models.iter().any(|model| model.side == Side::Agents && model.provider_id == each.id) {
                self.models.extend(before.models.iter().filter(|model| model.side == Side::Agents && model.provider_id == each.id).cloned());
            }
        }
        // (The names of the providers of what was kept.)
        for provider in &before.providers {
            if !self.providers.iter().any(|each| each.id == provider.id) && self.models.iter().any(|model| model.provider_id == provider.id) {
                self.providers.push(provider.clone());
            }
        }
        self
    }
}

fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<Option<ModelRef>>, D::Error> {
    Option::<ModelRef>::deserialize(deserializer).map(Some)
}

/// Where a new conversation goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartIn {
    /// A project, by its folder (or its name) on the box.
    Project(String),
    /// A chat, in no project.
    Chat,
}

/// How a new conversation starts, as its composer picked: in a worktree of its own, and in which mode
/// (none: this computer's default).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartHow {
    pub worktree: bool,
    pub approval: Option<crate::approval::Approval>,
    /// The computer picked in Runs on, by its key, where it isn't this box's own: the start goes to
    /// that computer's box, and says so, so a start sent to the wrong one is turned down there.
    pub computer: Option<String>,
}

/// Where a new session or bot can run, as the nearest box says it (`kuvo/setup/runs-on/events`,
/// PROTOCOL.md "Setup-wide list topics"): this computer first, then your others, each away one
/// still listed with when it was last seen.
pub const RUNS_ON_ROUTE: &str = "kuvo/setup/runs-on/events";

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RunsOn {
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub computers: Vec<RunsOnComputer>,
}

/// One computer Runs on lists.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RunsOnComputer {
    pub computer: crate::sessions::RowComputer,
    /// The box that said it: this computer.
    #[serde(default)]
    pub here: bool,
    /// It isn't answering now: shown, and not picked.
    #[serde(default)]
    pub away: bool,
    #[serde(default, rename = "lastSeen")]
    pub last_seen: Option<f64>,
    #[serde(default)]
    pub updating: bool,
}

impl BoxClient {
    /// Follows Runs on, as the box says it and again as it changes (on the line).
    pub fn follow_runs_on(&self) -> crate::follow::Follow<crate::follow::Followed<RunsOn>> {
        crate::follow::follow_lines(self, RUNS_ON_ROUTE.into())
    }

    /// Runs on, read once (a stand-in's: its events route answers its state and ends).
    #[cfg(feature = "stand-in")]
    pub fn runs_on_at_once(&self) -> Result<RunsOn> {
        let response = self.send(&Request::get(RUNS_ON_ROUTE))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        let text = String::from_utf8_lossy(&response.body);
        serde_json::from_str(text.lines().next().unwrap_or("{}")).map_err(|error| Error::BadAnswer(error.to_string()))
    }
}

/// What a new conversation is, as the box made it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Started {
    pub session: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    /// Its project's folder; none for a chat.
    #[serde(default)]
    pub project: Option<String>,
}

impl BoxClient {
    /// What `sessions`' box says of each one's model (at most 200 at once); a session it can't say
    /// about yet is left out.
    pub fn session_models(&self, sessions: &[&str]) -> Result<HashMap<String, ModelState>> {
        #[derive(Deserialize)]
        struct Told {
            sessions: HashMap<String, ModelState>,
        }
        let mut all = HashMap::new();
        for batch in sessions.chunks(200) {
            let list = batch.iter().map(|session| paths::query_value(session)).collect::<Vec<_>>().join(",");
            let told: Told = self.get(&format!("kuvo/sessions/models?sessions={list}"))?;
            all.extend(told.sessions);
        }
        Ok(all)
    }

    /// The models a picker offers here, and the one a new session starts on for `wanted` (the
    /// model picked last on this client), in `folder` (a project's or a conversation's: what its
    /// own settings add is offered there too, `models.offered.folder`).
    pub fn offered_models(&self, wanted: Option<&ModelRef>, folder: Option<&str>) -> Result<Offered> {
        self.offered_models_for_session(wanted, folder, None)
    }

    /// The box decides which models this conversation can use.
    pub fn offered_models_for_session(&self, wanted: Option<&ModelRef>, folder: Option<&str>, session: Option<&str>) -> Result<Offered> {
        self.read_offered_models(wanted, folder, session, false)
    }

    /// Reads the catalog again when a picker opens.
    pub fn refresh_offered_models(&self, wanted: Option<&ModelRef>, folder: Option<&str>, session: Option<&str>) -> Result<Offered> {
        self.read_offered_models(wanted, folder, session, true)
    }

    /// Models the box offers for its default and subagent settings.
    pub fn models_for_defaults(&self, refresh: bool) -> Result<Offered> {
        if self.has_feature(MODEL_DEFAULTS)? {
            let route = if refresh { "kuvo/models/offered?for=defaults&refresh=1" } else { "kuvo/models/offered?for=defaults" };
            let catalog = self.offered_answer(route)?;
            if !catalog.scoped {
                return Err(crate::Error::BadAnswer("The box didn't list the models it offers as defaults.".into()));
            }
            return Ok(catalog);
        }
        let mut legacy = self.read_offered_models(None, None, None, refresh)?;
        legacy.models.retain(|model| model.side == Side::Engine && model.is_offered());
        Ok(legacy)
    }

    fn read_offered_models(&self, wanted: Option<&ModelRef>, folder: Option<&str>, session: Option<&str>, refresh: bool) -> Result<Offered> {
        let mut query = Vec::new();
        if refresh {
            query.push("refresh=1".to_owned());
        }
        if let Some(session) = session {
            query.push(format!("session={}", paths::query_value(session)));
        }
        if let Some(folder) = folder.filter(|folder| !folder.is_empty()) {
            query.push(format!("folder={}", paths::query_value(folder)));
        }
        if let Some(wanted) = wanted {
            query.push(format!("provider={}&model={}", paths::query_value(&wanted.provider_id), paths::query_value(&wanted.id)));
            if let Some(variant) = &wanted.variant {
                query.push(format!("variant={}", paths::query_value(variant)));
            }
        }
        let path = if query.is_empty() { "kuvo/models/offered".to_owned() } else { format!("kuvo/models/offered?{}", query.join("&")) };
        self.offered_answer(&path)
    }

    fn offered_answer(&self, path: &str) -> Result<Offered> {
        let answer: Offered = self.get(path)?;
        if answer.starting == Some(None) && answer.start_choice.is_none() && self.advertised(MODEL_START_CHOICE) {
            return Err(crate::Error::BadAnswer("The box didn't say why a model must be chosen.".into()));
        }
        Ok(answer)
    }

    /// Stars `model` or takes its star away (`kuvo/models/starred`): your starred models now.
    pub fn set_starred(&self, model: &Starred, starred: bool) -> Result<Vec<Starred>> {
        #[derive(Deserialize)]
        struct Told {
            starred: Vec<Starred>,
        }
        let body = json!({ "providerID": model.provider_id, "id": model.id, "starred": starred });
        let told: Told = decode(&self.send(&Request::post("kuvo/models/starred", body))?)?;
        Ok(told.starred)
    }

    /// Puts `session` on `model` (with its effort as `variant`), from its next step.
    pub fn select_model(&self, session: &str, place: Where, model: &ModelRef) -> Result<()> {
        let body = json!({ "model": model });
        let response = self.send(&Request::post(place.session_route(session, "/model"), body))?;
        if response.is_success() { Ok(()) } else { Err(Error::Refused(Refusal::from_answer(response.status, &response.body))) }
    }

    /// Stops `session`'s run (`kuvo/sessions/<id>/stop`, `queue`): the box holds and pauses what
    /// waits to go, then stops the run, taking with it `on_their_way`, messages this client sent
    /// that may not have reached it yet (`queue.stop.arriving`): none waits here for them.
    pub fn stop_session(&self, session: &str, on_their_way: &[String]) -> Result<()> {
        // (Those waiting to be sent again that never got there don't go after it.)
        self.transport().take_back(on_their_way);
        let response = self.send(&Request::post(format!("kuvo/sessions/{}/stop", paths::segment(session)), json!({ "items": on_their_way })))?;
        if response.is_success() { Ok(()) } else { Err(Error::Refused(Refusal::from_answer(response.status, &response.body))) }
    }

    /// Starts an empty conversation (`sessions.start.empty`) in `place`, on `model` where one's
    /// named, for the user's own first message: as `how` says, in a worktree of its own (a
    /// project's: the box makes it, on a branch of its own) and in the mode picked.
    pub fn start_session(&self, place: &StartIn, model: Option<&ModelRef>, how: &StartHow) -> Result<Started> {
        let mut body = match place {
            StartIn::Project(project) => json!({ "project": project }),
            StartIn::Chat => json!({ "chat": true }),
        };
        if let Some(model) = model {
            body["model"] = json!(model);
        }
        if how.worktree && matches!(place, StartIn::Project(_)) {
            body["worktree"] = json!(true);
        }
        if let Some(approval) = &how.approval {
            body["approval"] = json!(approval.name());
        }
        if let Some(computer) = &how.computer {
            body["computer"] = json!(computer);
        }
        decode(&self.send(&Request::post("kuvo/sessions", body))?)
    }
}

/// An effort's name as a chip shows it ("xhigh" → "Extra"), as KuvoClient's `EffortLevel` does.
pub fn effort_title(id: &str) -> String {
    match id.to_lowercase().as_str() {
        "xhigh" | "x-high" | "extra-high" | "extra_high" | "extrahigh" => "Extra".into(),
        "max" | "maximum" => "Max".into(),
        "none" | "off" => "Off".into(),
        _ => {
            let mut chars = id.chars();
            chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn state(value: Value) -> ModelState {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn an_action_this_client_cant_read_leaves_the_rest() {
        let said = state(json!({ "model": null, "runs": false, "missing": "model", "refusal": "Choose a model to send it.", "choose": true, "action": { "do": "choose" } }));
        assert_eq!((said.refusal.as_deref(), said.choose, said.action), (Some("Choose a model to send it."), true, None));
    }

    #[test]
    fn what_the_box_didnt_say_is_drawn_as_running() {
        let mut gone = state(json!({ "model": { "providerID": "openai", "id": "gpt-4" }, "runs": false, "missing": "gone", "refusal": "gpt-4 is no longer available: choose another model.", "choose": true, "action": { "do": "choose", "label": "Choose a Model", "help": "Choose another model" } }));
        assert!(gone.choose);
        gone.unsaid();
        assert_eq!((gone.runs, gone.choose, gone.refusal, gone.action), (true, false, None, None));
    }

    fn offered(value: Value) -> Offered {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_side_that_cant_say_keeps_what_it_said_before() {
        let engine = json!({ "providerID": "anthropic", "id": "opus", "name": "Opus", "side": "engine", "efforts": ["low", "high"], "aliases": [] });
        let agents = json!({ "providerID": "claude-code", "id": "opus", "name": "Claude Code · Opus", "side": "agents", "efforts": [], "aliases": ["default"] });
        let before = offered(json!({ "models": [engine, agents], "known": { "engine": true, "agents": true }, "listing": ["codex"] }));
        // The bridge started again: its side isn't said now; what it said before stays.
        let now = offered(json!({ "models": [engine], "known": { "engine": true, "agents": false } })).keeping(Some(&before));
        assert_eq!(now.models.iter().map(|model| model.provider_id.as_str()).collect::<Vec<_>>(), ["anthropic"]);
        assert!(!now.is_whole());
        assert_eq!(now.models[0].efforts, ["low", "high"]);
        // The engine's, the same, ahead.
        let now = offered(json!({ "models": [agents], "known": { "engine": false, "agents": true } })).keeping(Some(&before));
        assert_eq!(now.models.iter().map(|model| model.side).collect::<Vec<_>>(), [Side::Engine, Side::Agents]);
        // Nothing before: as said, not whole.
        assert!(!offered(json!({ "models": [], "known": { "engine": false, "agents": true } })).keeping(None).is_whole());
    }

    #[test]
    fn a_failed_provider_keeps_only_its_own_previous_models_if_allowed() {
        let model = |provider: &str, id: &str| json!({ "providerID": provider, "id": id, "name": id, "side": "agents", "efforts": [], "aliases": [] });
        let before = offered(json!({ "models": [model("claude", "old"), model("codex", "kept"), model("grok", "other-account")], "generations": { "codex": "codex-A", "grok": "grok-A" }, "known": { "engine": true, "agents": true } }));
        let now = offered(json!({
            "models": [model("claude", "new")], "known": { "engine": true, "agents": true },
            "generations": { "codex": "codex-A", "grok": "grok-B" },
            "unavailable": [{ "id": "codex" }, { "id": "grok" }]
        })).keeping(Some(&before));
        assert_eq!(now.models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(), ["new", "kept"]);
        assert!(!now.is_whole());
    }

    #[test]
    fn offered_reads_whether_the_box_said_where_new_ones_start() {
        let said: Offered = serde_json::from_value(json!({ "models": [], "known": { "engine": true, "agents": true }, "starting": null })).unwrap();
        assert_eq!(said.starting, Some(None));
        let unsaid: Offered = serde_json::from_value(json!({ "models": [], "known": { "engine": false, "agents": true } })).unwrap();
        assert_eq!(unsaid.starting, None);
    }

    #[test]
    fn efforts_are_titled_as_every_client_titles_them() {
        assert_eq!(["low", "xhigh", "max", "none", "medium"].map(effort_title), ["Low", "Extra", "Max", "Off", "Medium"]);
    }
}
