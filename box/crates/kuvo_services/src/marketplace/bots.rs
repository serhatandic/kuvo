//! Marketplace bot scope projection (`.kuvo-marketplace.json` + `opencode.json` policies) and
//! agent bot helpers.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};


use super::{
    catalog::{CatalogAgent, CatalogStore},
    connections::{Connections, server_name},
    installations::{Installations, InstallationsStore},
};

pub const MARKETPLACE_SCOPE: &str = ".kuvo-marketplace.json";
const POLICY_RECORD: &str = ".kuvo-marketplace-policies.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceScope {
    pub agent: String,
    pub version: String,
    pub request: String,
    pub apps: Vec<String>,
    pub skills: Vec<String>,
}

pub fn marketplace_scope(directory: &Path) -> Option<MarketplaceScope> {
    let text = fs::read_to_string(directory.join(MARKETPLACE_SCOPE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn bots_dir_for(home: &Path) -> PathBuf {
    crate::config::box_env(home, "KUVO_BOTS").map(PathBuf::from).unwrap_or_else(|| home.join("bots"))
}

pub fn project_bot_scope(directory: &Path, scope: &MarketplaceScope, state: &Installations) -> Result<(), String> {
    let file = directory.join("opencode.json");
    let was = kuvo_store::files::text_of(&file);
    let mut config: Map<String, Value> = match &was {
        Some(text) => {
            let parsed: Value = serde_json::from_str(text).map_err(|_| "The workspace’s settings could not be read.".to_owned())?;
            let obj = parsed.as_object().cloned().ok_or_else(|| "The workspace’s settings could not be read.".to_owned())?;
            if obj.get("policies").is_some_and(|p| !p.is_array()) {
                return Err("The workspace’s settings could not be read.".into());
            }
            obj
        }
        None => Map::new(),
    };
    let record_file = directory.join(POLICY_RECORD);
    let previous: HashSet<String> = kuvo_store::files::text_of(&record_file)
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut policies: Vec<Value> = config.get("policies").and_then(Value::as_array).cloned().unwrap_or_default()
        .into_iter()
        .filter(|rule| !previous.contains(&serde_json::to_string(rule).unwrap_or_default()))
        .collect();
    let mut rules: Vec<Value> = vec![json!({ "action": "mcp.use", "resource": server_name("*"), "effect": "deny" })];
    for id in &scope.apps {
        rules.push(json!({ "action": "mcp.use", "resource": server_name(id), "effect": "allow" }));
    }
    for skill in &state.skills {
        if !scope.skills.contains(&skill.name) {
            rules.push(json!({ "action": "skill.use", "resource": skill.name, "effect": "deny" }));
        }
    }
    policies.extend(rules.clone());
    config.insert("policies".into(), Value::Array(policies));
    let mut output = serde_json::to_string_pretty(&Value::Object(config)).map_err(|e| e.to_string())?;
    output.push('\n');
    if was.as_deref() == Some(output.as_str()) { return Ok(()); }
    let projected: Vec<String> = rules.iter().map(|r| serde_json::to_string(r).unwrap_or_default()).collect();
    let mut combined: Vec<String> = previous.into_iter().collect();
    for p in &projected { if !combined.contains(p) { combined.push(p.clone()); } }
    kuvo_store::files::write_atomic(&record_file, &serde_json::to_vec(&combined).unwrap_or_default()).map_err(|e| e.to_string())?;
    kuvo_store::files::write_atomic(&file, output.as_bytes()).map_err(|e| e.to_string())?;
    kuvo_store::files::write_atomic(&record_file, &serde_json::to_vec(&projected).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn project_bot_scopes(bots_dir: &Path, state: &Installations) -> Vec<String> {
    let mut problems = Vec::new();
    for id in kuvo_store::files::list_of(bots_dir).into_iter().filter_map(|os| os.into_string().ok()) {
        if !id.starts_with("bot_") { continue; }
        let dir = bots_dir.join(&id).join("workspace");
        let Some(scope) = marketplace_scope(&dir) else { continue };
        if project_bot_scope(&dir, &scope, state).is_err() {
            problems.push(format!("Couldn’t refresh Marketplace tools for {id}."));
        }
    }
    problems
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentBotRef {
    pub id: String,
    pub name: String,
    pub computer: String,
}

pub fn local_agent_bots(bots_dir: &Path, here_key: &str) -> HashMap<String, Vec<AgentBotRef>> {
    let mut by_agent: HashMap<String, Vec<AgentBotRef>> = HashMap::new();
    for id in kuvo_store::files::list_of(bots_dir).into_iter().filter_map(|os| os.into_string().ok()) {
        if !id.starts_with("bot_") { continue; }
        let folder = bots_dir.join(&id);
        let Some(scope) = marketplace_scope(&folder.join("workspace")) else { continue };
        let name = kuvo_store::files::text_of(&folder.join("bot.json"))
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("name")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| id.clone());
        by_agent.entry(scope.agent).or_default().push(AgentBotRef { id, name, computer: here_key.into() });
    }
    by_agent
}

/// How this computer's bots are reached from here (its box's bots, given as the box starts): the
/// models it offers, a bot as it's listed, and a bot made with an agent's job and what goes in its
/// folder first.
pub trait AgentBots: Send + Sync {
    /// What the model picker offers here, starting on `wanted` where it runs; the engine started
    /// for it (`start`), else its models only while it runs. None where nothing can say.
    fn offered<'a>(&'a self, wanted: Option<&'a Value>, start: bool) -> futures_util::future::BoxFuture<'a, Option<Value>>;
    /// A bot here, as `kuvo/bots` lists it.
    fn listed(&self, id: &str) -> Option<Value>;
    /// Whether a bot on `model` runs with the coding agents (not the engine).
    fn runs_with_agents(&self, model: &Value) -> bool;
    /// `POST kuvo/bots` with `given`, and `setup`: its status and answer.
    fn make(&self, given: Value, setup: AgentSetup) -> futures_util::future::BoxFuture<'_, (u16, Value)>;
}

/// What's put in a bot's folder before its conversation is made: its folder, then the folder it
/// works in.
pub type Prepare = Box<dyn FnOnce(&Path, &Path) -> std::io::Result<()> + Send>;

/// What an agent's bot is made with besides what `POST kuvo/bots` takes: its job, and what's put
/// in its folder before its conversation is made (its folder, then the folder it works in).
pub struct AgentSetup {
    pub line: String,
    pub details: String,
    pub prepare: Prepare,
}

/// The apps an agent needs that aren't connected here, by their catalog titles.
pub fn missing_apps(agent: &CatalogAgent, catalog: &CatalogStore, installations: &InstallationsStore, connections: &Connections) -> Vec<Value> {
    let apps = installations.read().map(|s| s.apps).unwrap_or_default();
    let cat_apps = catalog.catalog().apps;
    agent.apps.iter().filter_map(|id| {
        let ready = apps.iter().find(|a| a.id == *id).is_some_and(|a| connections.connection_state(a) == "ready");
        if ready { return None; }
        let title = cat_apps.iter().find(|a| a.id == *id).map_or(id.as_str(), |a| a.title.as_str());
        Some(json!({ "id": id, "title": title }))
    }).collect()
}

/// What adding `agent` offers here: the models the picker offers that a bot can start on (not a
/// Custom Auto list, nor one retiring or kept out of its agent's picker), starting on its
/// suggested one where it runs; the apps still to connect; and its routines, each off.
pub async fn bot_options(agent: &CatalogAgent, bots: Option<&dyn AgentBots>, missing: Vec<Value>) -> Value {
    let wanted = json!({ "providerID": agent.suggested_model.provider_id, "id": agent.suggested_model.id });
    let offered = match bots {
        Some(bots) => bots.offered(Some(&wanted), false).await,
        None => None,
    };
    let mut answer = offered.unwrap_or_else(|| json!({ "models": [], "providers": [], "starred": [], "known": { "engine": false, "agents": false } }));
    if let Some(models) = answer["models"].as_array_mut() {
        models.retain(|model| model["providerID"] != "auto" && model["retiring"] != true && model["hidden"] != true);
    }
    let routines: Vec<Value> = agent.routines.iter().map(|r| {
        let mut obj = serde_json::to_value(r).unwrap_or_default();
        if let Some(m) = obj.as_object_mut() { m.insert("enabled".into(), Value::Bool(false)); }
        obj
    }).collect();
    answer["missing"] = json!(missing);
    answer["routines"] = json!(routines);
    answer
}

/// Whether `model` is one the picker offers here (by its id, or another it's known by).
pub fn offered_includes(offered: &Value, model: &Value) -> bool {
    offered["models"].as_array().is_some_and(|models| models.iter().any(|each| {
        each["providerID"] == model["providerID"]
            && (each["id"] == model["id"] || each["aliases"].as_array().is_some_and(|aliases| aliases.contains(&model["id"])))
    }))
}

/// A routine of an agent's, as a wake-up of its bot's: its title as its summary, due a day or a
/// week from now, and again as often.
pub fn seed_routine(folder: &Path, routine: &super::catalog::Routine, now_seconds: i64) -> std::io::Result<()> {
    let due = now_seconds + if routine.every == "1w" { 7 } else { 1 } * 86_400;
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(std::io::Error::other)?;
    let letters = hex::encode(random);
    let summaries = folder.join("summaries");
    fs::create_dir_all(&summaries)?;
    fs::write(summaries.join(&letters), &routine.title)?;
    fs::create_dir_all(folder.join("wakes"))?;
    fs::write(folder.join("wakes").join(format!("{due}-{letters}-{}", routine.every)), &routine.note)
}

pub fn valid_request_id(s: &str) -> bool {
    (22..=43).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
