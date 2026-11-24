//! All 27 `kuvo/marketplace*` routes, shared mutations, skill preview cache, and peer forwarding.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::StatusCode;
use kuvo_protocol::routes::*;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Value, json};

use crate::{
    config::{ConfigPaths, SettingsChanged, SettingsRead},
    skills::{
        library::SkillLibrary,
        names::{decode_uri_component, is_item_id, is_skill_folder, skill_about, skill_name, skill_trash_entry},
        store::{MARKETPLACE_SKILL, SkillFailure, SkillFile, SkillStore, local_skill, now_secs, sha256_hex},
    },
};

use super::{
    bots::{AgentBotRef, bot_options, bots_dir_for, local_agent_bots, project_bot_scopes, valid_request_id},
    catalog::{CatalogAgent, CatalogSkill, CatalogStore, is_endpoint},
    connections::{ConnectionError, Connections},
    installations::{
        Installations, InstallationsStore, InstalledApp, InstalledSkill, RemovedManaged, RemovedSkill,
        SaveOutcome, removal_for, skill_revision, valid_installed_app, valid_installed_skill,
    },
    skill_sources::{BuiltinOffer, BuiltinSkill, FolderSkill, archived_skill, engine_skill_offers, folder_skills, skill_roots},
};

const ROOT: &str = "kuvo/marketplace";
const YOUR_SKILLS: &str = "Your skills";
const MISSING: &str = "There’s no Marketplace item here.";

#[derive(Debug, Clone)]
pub struct MarketplaceComputer {
    pub key: String,
    pub name: String,
    pub here: bool,
    pub available: bool,
    pub supports_skill_removal: bool,
}

#[derive(Debug, Clone)]
pub struct PeerAct {
    pub method: String,
    pub item: String,
    pub body: Value,
}

pub type PeerActFuture<'a> = Pin<Box<dyn Future<Output = (u16, Value)> + Send + 'a>>;

pub trait MarketplacePeers: Send + Sync {
    fn computers(&self, here_name: &str) -> Vec<MarketplaceComputer>;
    fn peer_bots(&self) -> HashMap<String, Vec<AgentBotRef>>;
    fn forward<'a>(&'a self, peer_key: &'a str, act: PeerAct) -> PeerActFuture<'a>;
}

pub struct StandalonePeers;

impl MarketplacePeers for StandalonePeers {
    fn computers(&self, here_name: &str) -> Vec<MarketplaceComputer> {
        vec![MarketplaceComputer { key: "here".into(), name: here_name.into(), here: true, available: true, supports_skill_removal: true }]
    }
    fn peer_bots(&self) -> HashMap<String, Vec<AgentBotRef>> { HashMap::new() }
    fn forward<'a>(&'a self, _: &'a str, _: PeerAct) -> PeerActFuture<'a> {
        Box::pin(async { no(503, "Marketplace changes are waiting for the main computer.") })
    }
}

struct PreviewEntry { skill: InstalledSkill, expires: Instant }
struct SkillsHere { can_switch: bool, denied: HashSet<String>, deleted: Vec<Value>, places: Vec<String> }

pub struct MarketplaceService {
    pub home: PathBuf,
    pub account_home: PathBuf,
    pub is_mac: bool,
    pub computer_name: String,
    pub bots_dir: PathBuf,
    pub catalog: Arc<CatalogStore>,
    pub installations: Arc<InstallationsStore>,
    pub connections: Arc<Connections>,
    pub skills: Arc<SkillStore>,
    pub skill_library: Arc<SkillLibrary>,
    pub config: ConfigPaths,
    pub peers: Arc<dyn MarketplacePeers>,
    previews: Mutex<HashMap<String, PreviewEntry>>,
}

impl MarketplaceService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        home: &Path, account_home: &Path, is_mac: bool, computer_name: String,
        catalog: Arc<CatalogStore>, installations: Arc<InstallationsStore>,
        connections: Arc<Connections>, skills: Arc<SkillStore>,
        skill_library: Arc<SkillLibrary>, config: ConfigPaths, peers: Arc<dyn MarketplacePeers>,
    ) -> Self {
        Self {
            home: home.to_owned(), account_home: account_home.to_owned(), is_mac, computer_name,
            bots_dir: bots_dir_for(home), catalog, installations, connections, skills, skill_library,
            config, peers, previews: Mutex::new(HashMap::new()),
        }
    }

    pub fn reconcile(&self, state: Option<&Installations>, discovered: &[PathBuf]) -> Vec<String> {
        let owned;
        let state = match state {
            Some(s) => s,
            None => match self.installations.read() { Ok(s) => { owned = s; &owned } Err(e) => return vec![e] },
        };
        let mut p = self.installations.project_skills(Some(state), discovered);
        p.extend(self.connections.project_apps(Some(&state.apps)));
        p.extend(project_bot_scopes(&self.bots_dir, state));
        p
    }

    fn read_state(&self) -> Result<Installations, (u16, Value)> {
        self.installations.read().map_err(|e| no(502, &e))
    }

    fn files_on_this_box(&self, id: &str) -> bool {
        let folder = self.skills.skills_dir.join(id);
        folder.exists() && !folder.join(MARKETPLACE_SKILL).exists()
    }

    fn offer_skill(item: &CatalogSkill) -> InstalledSkill {
        InstalledSkill { name: item.id.clone(), files: item.files.clone(), source: None, origin: "catalog".into(), version: Some(item.version.clone()) }
    }

    fn catalog_app(&self, id: &str) -> Option<InstalledApp> {
        self.catalog.offered_apps().into_iter().find(|a| a.id == id).map(|e| InstalledApp {
            id: e.id, title: e.title, category: e.category, summary: e.summary, url: e.url, auth: e.auth, origin: "catalog".into(),
        })
    }

    fn app_card(&self, app: &InstalledApp, installed: bool, details: bool) -> Value {
        let conn = installed.then(|| self.connections.connection_state(app));
        let logo = (app.origin == "catalog" && self.catalog.offered_apps().iter().any(|a| a.id == app.id))
            .then(|| format!("{ROOT}/apps/{}/logo", app.id));
        let status = match (installed, conn) {
            (true, Some("ready")) => Some(json!({ "words": if app.auth == "open" { "Added" } else { "Connected" }, "tone": "good" })),
            (true, Some("locked")) => Some(json!({ "words": "Vault locked", "tone": "warning" })),
            _ => None,
        };
        let actions = if installed {
            let mut list = Vec::new();
            if app.auth != "open" {
                list.push(json!({ "id": "connect", "title": if conn == Some("ready") { "Reconnect" } else { "Connect" }, "enabled": conn != Some("locked") }));
                if conn == Some("ready") { list.push(json!({ "id": "disconnect", "title": "Disconnect on This Computer" })); }
            }
            list.push(json!({
                "id": "remove", "title": "Remove from All Computers", "style": "destructive",
                "confirm": { "title": format!("Remove {}?", app.title), "body": "It's removed from all your computers, with each computer's sign-in. Agents can't use it until you add it again.", "button": "Remove" }
            }));
            list
        } else {
            vec![json!({ "id": "add", "title": if app.auth == "open" { "Add" } else { "Connect" } })]
        };
        let mut out = json!({
            "id": app.id, "title": app.title, "category": app.category, "summary": app.summary,
            "origin": app.origin, "installed": installed, "actions": actions,
        });
        let m = out.as_object_mut().unwrap();
        if let Some(l) = logo { m.insert("logo".into(), json!(l)); }
        if let Some(c) = conn { m.insert("connection".into(), json!(c)); }
        if let Some(s) = status { m.insert("status".into(), s); }
        if details {
            m.insert("url".into(), json!(app.url));
            m.insert("auth".into(), json!(app.auth));
            m.insert("computerSignIn".into(), Value::Bool(app.auth != "open"));
            if app.auth == "token" {
                let mut form = json!({
                    "title": format!("Connect {}", app.title), "submit": "Connect",
                    "fields": [{ "id": "token", "title": "Personal access token", "kind": "password" }],
                });
                if app.origin == "catalog" && app.id == "github" {
                    form["help"] = json!("https://github.com/settings/personal-access-tokens/new");
                }
                m.insert("form".into(), form);
            }
        }
        out
    }

    fn computers_json(&self) -> Vec<Value> {
        self.peers.computers(&self.computer_name).into_iter().map(|c| {
            let mut v = json!({ "key": c.key, "name": c.name, "available": c.available });
            if c.here { v["here"] = Value::Bool(true); }
            v
        }).collect()
    }

    fn all_agent_bots(&self) -> HashMap<String, Vec<AgentBotRef>> {
        let here = self.peers.computers(&self.computer_name).into_iter().find(|c| c.here).map_or_else(|| "here".into(), |c| c.key);
        let mut by_id = HashMap::new();
        for (agent, list) in self.peers.peer_bots().into_iter().chain(local_agent_bots(&self.bots_dir, &here)) {
            for b in list { by_id.insert(b.id.clone(), (agent.clone(), b)); }
        }
        let mut out: HashMap<String, Vec<AgentBotRef>> = HashMap::new();
        for (agent, b) in by_id.into_values() { out.entry(agent).or_default().push(b); }
        out
    }

    async fn discover_skills(&self) -> (Vec<BuiltinOffer>, Vec<PathBuf>, Vec<FolderSkill>) {
        let (_, places) = skill_roots(&self.home, &self.account_home, &self.skills.skills_dir, &self.config);
        let (builtins, discovered) = engine_skill_offers(&self.skills.engine, &places).await;
        let folders = folder_skills(&self.home, &self.account_home, &self.skills.skills_dir, &self.config, &discovered, None);
        (builtins, discovered, folders)
    }

    fn skills_here(&self) -> Result<SkillsHere, String> {
        if !self.config.owns_settings() { return Err("This computer’s skills couldn’t be read.".into()); }
        let settings = match self.config.read_settings() {
            SettingsRead::Ok(m) => m,
            SettingsRead::Problem(p) => return Err(format!("The engine's settings file can't be changed from here: {p}")),
        };
        self.skills.clear_skill_trash(now_secs());
        let perms = settings.get("permissions");
        let denied = perms.and_then(Value::as_array).into_iter().flatten().filter_map(|r| {
            (opt(r, "action") == Some("skill") && !matches!(opt(r, "effect")?, "allow" | "ask"))
                .then(|| opt(r, "resource").map(str::to_owned)).flatten()
        }).collect();
        let places = settings.get("skills").and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)).collect()).unwrap_or_default();
        let deleted = self.skills.skill_trash_entries().into_iter()
            .map(|e| json!({ "id": str_at(&e, "id"), "title": str_at(&e, "name"), "summary": str_at(&e, "description") })).collect();
        Ok(SkillsHere { can_switch: perms.is_none() || perms.is_some_and(Value::is_array), denied, deleted, places })
    }

    pub async fn catalog_view(&self, query: &HashMap<String, String>) -> (u16, Value) {
        let state = match self.read_state() { Ok(s) => s, Err(e) => return e };
        let mut app_list = state.apps.clone();
        for a in self.catalog.offered_apps() {
            if !app_list.iter().any(|x| x.id == a.id) {
                app_list.push(InstalledApp { id: a.id, title: a.title, category: a.category, summary: a.summary, url: a.url, auth: a.auth, origin: "catalog".into() });
            }
        }
        let apps: Vec<Value> = app_list.iter().map(|a| self.app_card(a, state.apps.iter().any(|x| x.id == a.id), false)).collect();

        let cat = self.catalog.catalog();
        let cat_ids: HashSet<&str> = cat.skills.iter().map(|s| s.id.as_str()).collect();
        let inst_map: HashMap<&str, &InstalledSkill> = state.skills.iter().map(|s| (s.name.as_str(), s)).collect();
        let mut skill_cards: Vec<Value> = cat.skills.iter().map(|s| {
            let mut v = json!({ "id": s.id, "title": s.title, "category": s.category, "summary": s.summary, "version": s.version, "origin": "catalog", "installed": inst_map.contains_key(s.id.as_str()) });
            if let Some(inst) = inst_map.get(s.id.as_str()) {
                if let Some(ver) = &inst.version { v["installedVersion"] = json!(ver); }
            } else { v["actions"] = json!([{ "id": "add", "title": "Install" }]); }
            v
        }).collect();

        for s in &state.skills {
            if s.origin == "catalog" && cat_ids.contains(s.name.as_str()) { continue; }
            let mut card = card_skill(s, true);
            card["title"] = json!(opt(&card, "name").unwrap_or(&s.name));
            card["summary"] = json!(str_at(&card, "description"));
            card["category"] = json!(YOUR_SKILLS);
            skill_cards.push(card);
        }

        let (builtins, _, folders) = self.discover_skills().await;
        let rem_names: HashSet<&str> = state.removed_skills.as_deref().into_iter().flatten()
            .filter(|r| r.restored != Some(true)).map(|r| r.name.as_str()).collect();
        let mut occupied: HashSet<String> = inst_map.keys().chain(cat_ids.iter()).map(|s| (*s).to_owned()).collect();
        for f in folders {
            let base = f.folder.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if rem_names.contains(f.id.as_str()) || rem_names.contains(base.as_str()) || !occupied.insert(f.id.clone()) { continue; }
            skill_cards.push(json!({
                "id": f.id, "title": f.title, "summary": f.summary, "location": f.folder.to_string_lossy(),
                "category": YOUR_SKILLS, "origin": "found", "installed": true, "actions": [remove_action(&f.title, false)],
            }));
        }
        let mut bi_list: Vec<(BuiltinSkill, bool)> = builtins.into_iter().map(|b| { let r = removal_for(&state, &b.skill.id).is_some(); (b.skill, r) }).collect();
        for rem in state.removed_skills.as_deref().into_iter().flatten() {
            if rem.restored != Some(true) && let Some(b) = &rem.builtin { bi_list.push((b.clone(), true)); }
        }
        for (b, is_rem) in bi_list {
            if !occupied.insert(b.id.clone()) { continue; }
            skill_cards.push(json!({
                "id": b.id, "title": b.title, "summary": b.summary, "location": "Built in",
                "category": if is_rem { "Built In" } else { YOUR_SKILLS }, "origin": if is_rem { "catalog" } else { "found" },
                "installed": !is_rem, "actions": if is_rem { json!([{ "id": "add", "title": "Install" }]) } else { json!([remove_action(&b.title, true)]) },
            }));
        }

        let here = self.skills_here();
        if let Ok(ref h) = here {
            for item in &mut skill_cards {
                if item.get("installed").and_then(Value::as_bool) == Some(true) {
                    item["enabled"] = Value::Bool(!h.denied.contains(str_at(item, "id")));
                    if !h.can_switch { item["switchWhy"] = json!("Permissions are set as something other than a list. Change them in the settings file."); }
                }
            }
        }
        let here_json = match here {
            Err(problem) => json!({ "problem": problem }),
            Ok(h) => {
                let mut seen_folders = HashSet::new();
                let deleted: Vec<Value> = h.deleted.into_iter().filter(|item| {
                    let Some(arch) = archived_skill(&self.skills.trash_dir, str_at(item, "id")) else { return true };
                    removal_for(&state, &arch.name).is_some_and(|r| r.key == arch.key) && seen_folders.insert(arch.folder)
                }).collect();
                let places: Vec<Value> = h.places.iter().enumerate().map(|(i, p)| json!({ "id": i.to_string(), "title": p })).collect();
                json!({
                    "computer": self.computer_name, "switchTitle": format!("Use on {}", self.computer_name),
                    "deleted": { "title": "Recently Deleted", "note": "Deleted skills are kept for 30 days, then removed for good.", "items": deleted, "action": { "id": "restore", "title": "Restore" } },
                    "places": {
                        "title": "Other Places", "note": "A folder, or the address of a skill catalog with an index.json, where agents also find skills.",
                        "items": places, "remove": { "id": "remove", "title": "Remove" },
                        "add": { "id": "add", "title": "Add Place", "form": { "title": "Add a Place", "submit": "Add", "fields": [{ "id": "place", "title": "A folder, or the address of a skill catalog" }] } },
                    }
                })
            }
        };

        let added_bots = self.all_agent_bots();
        let agents: Vec<Value> = self.catalog.offered_agents().into_iter().map(|a| {
            let bot_count = added_bots.get(&a.id).map_or(0, Vec::len);
            let mut parts: Vec<String> = a.apps.iter().map(|id| cat.apps.iter().find(|x| x.id == *id).map_or(id.clone(), |x| x.title.clone())).collect();
            parts.push(plural(a.skills.len(), "skill"));
            if bot_count > 0 { parts.push(format!("{} added", plural(bot_count, "bot"))); }
            json!({
                "id": a.id, "title": a.title, "category": a.category, "summary": a.summary, "version": a.version,
                "apps": a.apps, "skills": a.skills, "suggestedModel": a.suggested_model, "routines": a.routines,
                "botCount": bot_count, "detail": parts.join(" · "), "actions": [{ "id": "add", "title": "Add" }],
            })
        }).collect();

        let mut skills_area = build_area("skills", "Skills", skill_cards, query, json!([
            { "id": "preview", "title": "Add from Link or Folder", "form": preview_form() },
            { "id": "create", "title": "New Skill", "form": create_form() },
        ]));
        skills_area["here"] = here_json;

        (200, json!({
            "title": "Marketplace", "computers": self.computers_json(), "searchPlaceholder": "Search, or paste a GitHub link",
            "areas": [
                build_area("apps", "Apps", apps, query, json!([{
                    "id": "custom", "title": "Add Custom",
                    "form": { "title": "Add Custom App", "submit": "Add", "fields": [
                        { "id": "title", "title": "Name" },
                        { "id": "url", "title": "MCP server address", "placeholder": "https://example.com/mcp" },
                        { "id": "auth", "title": "Sign-in", "choices": [
                            { "id": "open", "title": "No sign-in" }, { "id": "token", "title": "Personal access token" },
                            { "id": "oauth", "title": "Sign in with the service" },
                        ] },
                    ] }
                }])),
                build_area("agents", "Agents", agents, query, json!([])),
                skills_area,
            ],
        }))
    }

    pub async fn shared_mutation(&self, item: &str, body: &Value) -> (u16, Value) {
        let res = self.installations.save(|mut state| match item {
            "save-app" => {
                let app: InstalledApp = serde_json::from_value(body.get("app").cloned().unwrap_or(Value::Null)).map_err(|_| no(404, MISSING))?;
                if !valid_installed_app(&app) { return Err(no(404, MISSING)); }
                if let Some(b) = state.apps.iter().find(|a| a.id == app.id) {
                    return Err(if b.url == app.url && b.auth == app.auth && b.origin == app.origin { (200, json!({ "installed": true })) } else { no(409, "An app already uses that name.") });
                }
                if state.apps.len() >= 60 { return Err(no(413, "Remove an app before adding another.")); }
                state.apps.push(app);
                Ok(state)
            }
            "save-skill" => {
                let skill: InstalledSkill = serde_json::from_value(body.get("skill").cloned().unwrap_or(Value::Null)).map_err(|_| no(404, MISSING))?;
                if !valid_installed_skill(&skill) { return Err(no(404, MISSING)); }
                let old = state.skills.iter().find(|s| s.name == skill.name);
                if old.is_some_and(|o| skill_revision(o) == skill_revision(&skill)) { return Err((200, json!({ "installed": true }))); }
                if old.map(skill_revision).as_deref() != opt(body, "was") { return Err(no(409, "The installed skill changed. Read it again first.")); }
                if self.files_on_this_box(&skill.name) { return Err(no(409, "A skill with that name already exists on the main computer.")); }
                state.skills.retain(|s| s.name != skill.name);
                if let Some(rems) = &mut state.removed_skills { rems.retain(|r| r.name != skill.name); }
                state.skills.push(skill);
                Ok(state)
            }
            "agent-skills" => {
                let (id, ver) = (str_at(body, "id"), str_at(body, "version"));
                let Some(agent) = self.catalog.offered_agents().into_iter().find(|a| a.id == id && a.version == ver) else {
                    return Err(no(409, "This agent changed. Read it again before adding it."));
                };
                let cat = self.catalog.catalog();
                let add: Vec<InstalledSkill> = agent.skills.iter().filter(|sid| !state.skills.iter().any(|s| s.name == **sid))
                    .filter_map(|sid| cat.skills.iter().find(|s| s.id == *sid).map(Self::offer_skill)).collect();
                if add.iter().any(|s| self.files_on_this_box(&s.name)) { return Err(no(409, "A skill required by this agent already exists as a local skill on the main computer.")); }
                if add.is_empty() { return Err((200, json!({ "installed": true }))); }
                if let Some(rems) = &mut state.removed_skills { rems.retain(|r| !add.iter().any(|a| a.name == r.name)); }
                state.skills.extend(add);
                Ok(state)
            }
            "remove-app" => {
                let id = opt(body, "id").filter(|s| is_item_id(s)).ok_or_else(|| no(404, MISSING))?;
                if !state.apps.iter().any(|a| a.id == id) { return Err((200, json!({ "installed": false }))); }
                state.apps.retain(|a| a.id != id);
                Ok(state)
            }
            "remove-skill" | "remove-skill-everywhere" => {
                let id = opt(body, "id").filter(|s| is_skill_folder(s)).ok_or_else(|| no(404, MISSING))?;
                let old = state.skills.iter().find(|s| s.name == id).cloned();
                if removal_for(&state, id).is_some() || (item == "remove-skill" && old.is_none()) { return Err((200, json!({ "installed": false }))); }
                let ok = match &old { Some(o) => opt(body, "was") == Some(skill_revision(o).as_str()), None => body.get("managed").and_then(Value::as_bool) != Some(true) };
                if !ok { return Err(no(409, "The installed skill changed. Read it again first.")); }
                let builtin = body.get("builtin").and_then(|v| serde_json::from_value::<BuiltinSkill>(v.clone()).ok())
                    .filter(|b| b.id == id && !b.title.is_empty() && b.title.len() <= 1024 && b.summary.len() <= 4096);
                let removed = RemovedSkill {
                    name: id.into(), key: random_uuid_v4(), restored: None, builtin,
                    managed: old.map(|o| RemovedManaged { origin: o.origin, version: o.version, source: o.source }),
                };
                state.skills.retain(|s| s.name != id);
                let rems = state.removed_skills.get_or_insert_with(Vec::new);
                rems.retain(|r| r.name != id);
                rems.push(removed);
                Ok(state)
            }
            "restore-skill" => {
                let id = opt(body, "id").filter(|s| is_skill_folder(s)).ok_or_else(|| no(404, MISSING))?;
                let rem = removal_for(&state, id).filter(|r| opt(body, "key") == Some(r.key.as_str())).cloned()
                    .ok_or_else(|| no(409, "This skill’s removal changed. Read it again first."))?;
                let restored_skill = if let Some(m) = &rem.managed {
                    let files: Vec<SkillFile> = serde_json::from_value(body.get("files").cloned().unwrap_or(Value::Null)).map_err(|_| no(422, "The archived skill’s files could not be verified."))?;
                    let s = InstalledSkill { name: id.into(), files, source: m.source.clone(), origin: m.origin.clone(), version: m.version.clone() };
                    if !valid_installed_skill(&s) { return Err(no(422, "The archived skill’s files could not be verified.")); }
                    Some(s)
                } else { None };
                if state.skills.iter().any(|s| s.name == id) { return Err(no(409, "A skill already uses that name.")); }
                if let Some(s) = restored_skill { state.skills.push(s); }
                if let Some(rems) = &mut state.removed_skills {
                    for r in rems.iter_mut() {
                        if r.name == rem.name && r.key == rem.key { *r = RemovedSkill { name: r.name.clone(), key: r.key.clone(), restored: Some(true), builtin: None, managed: None }; }
                    }
                }
                Ok(state)
            }
            _ => Err(no(404, MISSING)),
        }).await;

        match res {
            SaveOutcome::Saved { state, receipt } => {
                let mut out = json!({ "problems": self.reconcile(Some(&state), &[]) });
                if let Some(r) = receipt { out["receipt"] = r; }
                (200, out)
            }
            SaveOutcome::Done { status: 200, mut value } => {
                if let Some(m) = value.as_object_mut() { m.insert("problems".into(), json!(self.reconcile(None, &[]))); }
                (200, value)
            }
            SaveOutcome::Done { status, value } => (status, value),
        }
    }

    async fn save(&self, item: &str, body: Value, places: &[PathBuf]) -> (u16, Value) {
        if let Some(setup) = self.installations.setup_state() && !setup.main_here {
            let comps = self.peers.computers(&self.computer_name);
            let Some(p) = comps.iter().find(|c| c.key == setup.primary) else {
                return no(503, "Marketplace changes are waiting for the main computer.");
            };
            if matches!(item, "remove-skill-everywhere" | "restore-skill") && !p.supports_skill_removal {
                return no(409, &format!("Update Kuvo Code on {} to remove or restore setup-wide skills.", p.name));
            }
            if !p.available { return no(409, &format!("Update Kuvo Code on {} to use Marketplace there.", p.name)); }
            return self.peers.forward(&p.key, PeerAct { method: "POST".into(), item: item.into(), body }).await;
        }
        let (st, mut val) = self.shared_mutation(item, &body).await;
        if st == 200 && !places.is_empty() && let Some(m) = val.as_object_mut() {
            m.insert("problems".into(), json!(self.reconcile(None, places)));
        }
        (st, val)
    }

    async fn save_skill_with_id(&self, sk: InstalledSkill, was: Option<&Value>) -> (u16, Value) {
        if self.files_on_this_box(&sk.name) { return no(409, "A local skill already uses that name."); }
        let id = sk.name.clone();
        let (st, mut v) = self.save("save-skill", json!({ "skill": sk, "was": was }), &[]).await;
        if let Some(m) = v.as_object_mut() { m.insert("id".into(), json!(id)); }
        (st, v)
    }

    pub async fn marketplace_for_peer(&self, act: PeerAct) -> (u16, Value) {
        if act.method == "POST" && matches!(act.item.as_str(), "save-app" | "save-skill" | "remove-app" | "remove-skill" | "remove-skill-everywhere" | "restore-skill" | "agent-skills") {
            if self.installations.setup_state().is_none_or(|s| !s.main_here) { return no(409, "Marketplace changes are waiting for the main computer."); }
            return self.shared_mutation(&act.item, &act.body).await;
        }
        if let Some(id) = act.item.strip_prefix("options/") && act.method == "GET" {
            return self.catalog.offered_agents().into_iter().find(|a| a.id == id)
                .map_or_else(|| no(404, MISSING), |a| (200, bot_options(&a, &self.catalog, &self.installations, &self.connections)));
        }
        if let Some(id) = act.item.strip_prefix("add-agent/") && act.method == "POST" {
            return self.catalog.offered_agents().into_iter().find(|a| a.id == id)
                .map_or_else(|| no(404, MISSING), |a| self.add_agent_local(&a, &act.body));
        }
        no(404, MISSING)
    }

    fn stage_preview(&self, skill: InstalledSkill) -> (u16, Value) {
        let (now, id) = (Instant::now(), skill_revision(&skill));
        if let Ok(mut map) = self.previews.lock() {
            map.retain(|_, v| v.expires > now);
            if map.len() >= 12 && let Some(k) = map.keys().next().cloned() { map.remove(&k); }
            map.insert(id.clone(), PreviewEntry { skill: skill.clone(), expires: now + Duration::from_secs(15 * 60) });
        }
        (200, json!({ "preview": id, "skill": card_skill(&skill, false), "installed": false, "files": skill.files, "action": { "id": "install", "title": "Install Skill" } }))
    }

    fn find_preview(&self, raw: Option<&str>) -> Option<InstalledSkill> {
        let id = raw.filter(|s| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))?;
        self.previews.lock().ok()?.get(id).filter(|e| e.expires > Instant::now()).map(|e| e.skill.clone())
    }

    fn add_agent_local(&self, agent: &CatalogAgent, body: &Value) -> (u16, Value) {
        let problems = self.reconcile(None, &[]);
        if !problems.is_empty() { return no(409, &problems[0]); }
        let model_ok = body.get("model").is_some_and(|m| !str_at(m, "id").is_empty() && !matches!(str_at(m, "providerID"), "" | "auto"));
        let routines_ok = body.get("routines").is_none_or(|v| v.as_array().is_some_and(|a| a.iter().all(|id| id.as_str().is_some_and(|s| agent.routines.iter().any(|r| r.id == s)))));
        if !valid_request_id(str_at(body, "request")) || !model_ok || !body.get("routine").is_none_or(Value::is_boolean) || !routines_ok {
            return no(400, "Choose a model and account, and send the original Add request ID.");
        }
        if opt(body, "version") != Some(agent.version.as_str()) {
            return no(409, "This agent changed. Open its details and review the new version.");
        }
        let state = match self.read_state() { Ok(s) => s, Err(e) => return e };
        let here = if self.is_mac { "this Mac" } else { "this machine" };
        if agent.apps.iter().any(|id| !state.apps.iter().any(|a| a.id == *id && self.connections.connection_state(a) == "ready")) {
            return no(409, &format!("Connect the required apps on {here} before adding this agent."));
        }
        if agent.skills.iter().any(|id| !state.skills.iter().any(|s| s.name == *id)) {
            return no(409, "This computer is still receiving the required skills. Try again shortly.");
        }
        no(422, "Choose a model and account available on this computer.")
    }

    fn require_app(&self, id: &str) -> Result<InstalledApp, (u16, Value)> {
        let state = self.read_state()?;
        if let Some(app) = state.apps.into_iter().find(|a| a.id == id) { return Ok(app); }
        if self.catalog_app(id).is_some() {
            return Err(no(409, "Add the app first. Wait for the setup to deliver the change if you added it on another computer."));
        }
        Err(no(404, MISSING))
    }

    fn edit_places(&self, f: impl FnOnce(&mut Vec<Value>)) -> (u16, Value) {
        let before = match self.config.read_settings() { SettingsRead::Ok(m) => m.get("skills").cloned(), SettingsRead::Problem(p) => return no(409, &p) };
        let mut places = before.as_ref().and_then(Value::as_array).cloned().unwrap_or_default();
        f(&mut places);
        match self.config.change_settings(&json!([{ "path": ["skills"], "value": places, "was": before.unwrap_or(Value::Null) }])) {
            SettingsChanged::Ok(_) => (200, json!({})),
            SettingsChanged::Err { status, message, .. } => no(status, &message),
        }
    }

    async fn skill_get(&self, id: &str) -> (u16, Value) {
        let state = match self.read_state() { Ok(s) => s, Err(e) => return e };
        let skill = state.skills.iter().find(|x| x.name == id);
        let listed = self.catalog.catalog().skills.into_iter().find(|x| x.id == id);
        if skill.is_some() || listed.is_some() {
            let info = skill.cloned().unwrap_or_else(|| Self::offer_skill(listed.as_ref().unwrap()));
            let mut card = card_skill(&info, skill.is_some());
            let title = listed.as_ref().map(|l| l.title.clone()).or_else(|| opt(&card, "name").map(str::to_owned)).unwrap_or_else(|| id.into());
            let summary = listed.as_ref().map(|l| l.summary.clone()).or_else(|| opt(&card, "description").map(str::to_owned));
            let mut actions = Vec::new();
            if let Some(sk) = skill {
                actions.push(remove_action(&title, false));
                if sk.source.is_some() { actions.push(json!({ "id": "update-preview", "title": "Check for Update" })); }
                if sk.source.is_none() && sk.origin != "catalog" { actions.push(json!({ "id": "edit", "title": "Edit", "form": edit_form(&title, &skill_text(sk)) })); }
            } else { actions.push(json!({ "id": "add", "title": "Install Skill" })); }
            card["title"] = json!(title);
            if let Some(sum) = summary { card["summary"] = json!(sum); }
            card["category"] = json!(listed.as_ref().map_or(YOUR_SKILLS, |l| l.category.as_str()));
            card["files"] = json!(info.files);
            card["actions"] = Value::Array(actions);
            if let Some(l) = &listed { card["available"] = json!({ "version": l.version, "revision": skill_revision(&Self::offer_skill(l)), "files": l.files }); }
            return (200, card);
        }
        if let Some(rem) = removal_for(&state, id) {
            return match &rem.builtin {
                Some(b) => (200, json!({ "id": b.id, "title": b.title, "summary": b.summary, "category": "Built In", "origin": "catalog", "installed": false, "available": builtin_available(b), "actions": [{ "id": "add", "title": "Install Skill" }] })),
                None => no(404, MISSING),
            };
        }
        let (builtins, _, folders) = self.discover_skills().await;
        if let Some(f) = folders.into_iter().find(|x| x.id == id) {
            let own = f.folder == self.skills.skills_dir.join(id);
            let source = own.then(|| self.skills.skill_sources().remove(id)).flatten();
            let mut actions = vec![remove_action(&f.title, false)];
            if own && source.is_none() && !f.text.is_empty() { actions.push(json!({ "id": "edit", "title": "Edit", "form": edit_form(&f.title, &f.text) })); }
            let mut out = json!({
                "id": id, "title": f.title, "summary": f.summary, "category": YOUR_SKILLS,
                "origin": if own { "local" } else { "found" }, "location": f.folder.to_string_lossy(),
                "installed": true, "revision": folder_rev(&f.folder, &f.text), "actions": actions,
                "files": [skill_md_file(&f.text)],
            });
            if let Some(src) = source { out["source"] = src; }
            return (200, out);
        }
        if let Some(b) = builtins.into_iter().find(|x| x.skill.id == id) {
            let files: Vec<Value> = b.text.as_deref().map(skill_md_file).into_iter().collect();
            return (200, json!({ "id": b.skill.id, "title": b.skill.title, "summary": b.skill.summary, "category": YOUR_SKILLS, "origin": "found", "location": "Built in", "installed": true, "revision": builtin_rev(id), "files": files, "actions": [remove_action(&b.skill.title, true)] }));
        }
        no(404, MISSING)
    }

    async fn skill_patch(&self, id: &str, body: &Value) -> (u16, Value) {
        let state = match self.read_state() { Ok(s) => s, Err(e) => return e };
        if let Some(mut skill) = state.skills.iter().find(|x| x.name == id).cloned() {
            if skill.source.is_some() || skill.origin == "catalog" { return no(409, "Edit a copy of this skill or install an updated version."); }
            let Some(edit) = written_skill(body, Some(id), &skill_text(&skill)) else { return no(400, "Give the skill a description and instructions."); };
            for f in &mut skill.files { if f.path == "SKILL.md" { *f = edit.files[0].clone(); } }
            return self.save("save-skill", json!({ "skill": skill, "was": body.get("was") }), &[]).await;
        }
        if removal_for(&state, id).is_some() { return no(404, MISSING); }
        let (_, _, folders) = self.discover_skills().await;
        let Some(f) = folders.into_iter().find(|x| x.id == id && x.folder == self.skills.skills_dir.join(id)) else { return no(404, MISSING); };
        if opt(body, "was") != Some(folder_rev(&f.folder, &f.text).as_str()) { return no(409, "The installed skill changed. Read it again first."); }
        let (Some(desc), Some(instr)) = (opt(body, "description").filter(|d| !d.trim().is_empty()), opt(body, "instructions")) else {
            return no(400, "Give the skill a description and instructions.");
        };
        let file = f.folder.join("SKILL.md");
        if kuvo_store::files::text_of(&file).as_deref() != Some(f.text.as_str()) { return no(409, "It changed since you read it."); }
        let doc = skill_document(skill_about(&f.text).name.as_deref().unwrap_or(id), desc, instr, &f.text);
        match kuvo_store::files::write_atomic(&file, doc.as_bytes()) { Ok(()) => (200, json!({})), Err(e) => no(502, &e.to_string()) }
    }

    async fn skill_delete(&self, id: &str, body: &Value) -> (u16, Value) {
        let state = match self.read_state() { Ok(s) => s, Err(e) => return e };
        if state.skills.iter().any(|x| x.name == id) {
            return self.save("remove-skill-everywhere", json!({ "id": id, "was": body.get("was"), "managed": true }), &[]).await;
        }
        if removal_for(&state, id).is_some() { return no(404, MISSING); }
        let (builtins, places, folders) = self.discover_skills().await;
        if let Some(f) = folders.into_iter().find(|x| x.id == id) {
            if opt(body, "was") != Some(folder_rev(&f.folder, &f.text).as_str()) { return no(409, "The installed skill changed. Read it again first."); }
            return self.save("remove-skill-everywhere", json!({ "id": id }), &places).await;
        }
        if let Some(b) = builtins.into_iter().find(|x| x.skill.id == id) {
            if opt(body, "was") != Some(builtin_rev(id).as_str()) { return no(409, "The installed skill changed. Read it again first."); }
            return self.save("remove-skill-everywhere", json!({ "id": id, "builtin": b.skill }), &places).await;
        }
        no(404, MISSING)
    }

    async fn skill_restore(&self, body: &Value) -> (u16, Value) {
        self.skills.clear_skill_trash(now_secs());
        let id = str_at(body, "id");
        if let Some(arch) = archived_skill(&self.skills.trash_dir, id) {
            let state = match self.read_state() { Ok(s) => s, Err(e) => return e };
            let Some(removed) = removal_for(&state, &arch.name).filter(|r| r.key == arch.key) else {
                return no(409, "This skill has already been restored or installed again.");
            };
            if std::fs::symlink_metadata(&arch.folder).is_ok() { return no(409, "A skill already uses that folder."); }
            let files = if removed.managed.is_some() {
                match local_skill(&self.skills.trash_dir.join(id)) { Ok(pkg) => Some(pkg.files), Err(e) => return no(e.status, &e.message) }
            } else { None };
            return self.save("restore-skill", json!({ "id": arch.name, "key": arch.key, "files": files }), &[]).await;
        }
        let Some(parsed) = skill_trash_entry(id) else { return no(400, "That isn’t an entry in Recently Deleted."); };
        let away = self.skills.trash_dir.join(id);
        if !std::fs::metadata(&away).is_ok_and(|m| m.is_dir()) { return (404, json!({})); }
        let back = self.skills.skills_dir.join(&parsed.folder);
        if back.exists() { return no(409, &format!("There’s a skill called {} now. Delete it first to restore this one.", parsed.folder)); }
        match std::fs::create_dir_all(&self.skills.skills_dir).and_then(|()| std::fs::rename(&away, &back)) {
            Ok(()) => (200, json!({})),
            Err(e) => no(502, &e.to_string()),
        }
    }
}

fn no(status: u16, msg: &str) -> (u16, Value) { (status, json!({ "message": msg })) }
fn opt<'a>(v: &'a Value, k: &str) -> Option<&'a str> { v.get(k).and_then(Value::as_str) }
fn str_at<'a>(v: &'a Value, k: &str) -> &'a str { opt(v, k).unwrap_or("") }
fn folder_rev(folder: &Path, text: &str) -> String { sha256_hex(serde_json::to_string(&json!([folder.to_string_lossy(), text])).unwrap_or_default().as_bytes()) }
fn builtin_rev(id: &str) -> String { sha256_hex(serde_json::to_string(&json!([id, "built-in"])).unwrap_or_default().as_bytes()) }
fn skill_md_file(text: &str) -> Value { json!({ "path": "SKILL.md", "data": STANDARD.encode(text.as_bytes()), "sha256": sha256_hex(text.as_bytes()) }) }
fn pair_refusal((st, v): (u16, Value)) -> Refusal { Refusal::json(StatusCode::from_u16(st).unwrap_or(StatusCode::BAD_GATEWAY), v) }

fn card_skill(skill: &InstalledSkill, installed: bool) -> Value {
    let about = skill_about(&skill_text(skill));
    let mut v = json!({ "id": skill.name, "origin": skill.origin, "revision": skill_revision(skill), "installed": installed });
    if let Some(n) = about.name { v["name"] = json!(n); }
    if let Some(d) = about.description { v["description"] = json!(d); }
    if let Some(ver) = &skill.version { v["version"] = json!(ver); }
    if let Some(s) = &skill.source { v["source"] = json!(s); }
    v
}

fn skill_text(skill: &InstalledSkill) -> String {
    skill.files.iter().find(|f| f.path == "SKILL.md")
        .and_then(|f| STANDARD.decode(&f.data).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

pub fn skill_document(name: &str, description: &str, body: &str, existing: &str) -> String {
    let mut others = Vec::new();
    let norm = existing.replace("\r\n", "\n");
    if let Some(rest) = norm.strip_prefix("---\n") && let Some(end) = rest.find("\n---") {
        let mut skip = false;
        for line in rest[..end].lines() {
            if line.starts_with("name:") || line.starts_with("description:") { skip = true; continue; }
            if skip && line.starts_with(char::is_whitespace) { continue; }
            skip = false;
            if !line.is_empty() { others.push(line); }
        }
    }
    let extra = if others.is_empty() { String::new() } else { format!("{}\n", others.join("\n")) };
    format!("---\nname: {}\ndescription: {}\n{extra}---\n\n{}\n", serde_json::to_string(name).unwrap_or_default(), serde_json::to_string(description.trim()).unwrap_or_default(), body.trim())
}

fn strip_frontmatter(md: &str) -> String {
    let norm = md.replace("\r\n", "\n");
    norm.strip_prefix("---\n").and_then(|r| r.find("\n---").map(|e| r[e + 4..].trim().to_owned())).unwrap_or_else(|| norm.trim().to_owned())
}

fn written_skill(body: &Value, name: Option<&str>, existing: &str) -> Option<InstalledSkill> {
    let desc = opt(body, "description").filter(|s| !s.trim().is_empty())?;
    let instr = opt(body, "instructions")?;
    let resolved = name.map(str::to_owned).unwrap_or_else(|| skill_name(str_at(body, "name")));
    if !is_item_id(&resolved) { return None; }
    let doc = skill_document(&resolved, desc, instr, existing);
    let b = doc.as_bytes();
    (b.len() <= 64 * 1024).then(|| InstalledSkill {
        name: resolved, source: None, origin: "created".into(), version: None,
        files: vec![SkillFile { path: "SKILL.md".into(), data: STANDARD.encode(b), sha256: sha256_hex(b) }],
    })
}

fn field(id: &str, title: &str, line: &str, extra: Value, val: Option<String>) -> Value {
    let mut v = json!({ "id": id, "title": title, "kind": "lines", "line": line });
    if let (Some(dst), Some(src)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in src { dst.insert(k.clone(), x.clone()); }
        if let Some(val) = val { dst.insert("value".into(), json!(val)); }
    }
    v
}
fn use_when(v: Option<String>) -> Value { field("description", "Use it when", "The agent reads this to decide when the skill is needed, and loads the instructions only then.", json!({ "placeholder": "Writing release notes from the commits since the last tag" }), v) }
fn instr_field(v: Option<String>) -> Value { field("instructions", "Instructions", "Markdown. Mention any other files in the skill so the agent knows to use them.", json!({ "mono": true }), v) }
fn edit_form(title: &str, md: &str) -> Value { json!({ "title": format!("Edit {title}"), "submit": "Save", "fields": [use_when(Some(skill_about(md).description.unwrap_or_default())), instr_field(Some(strip_frontmatter(md)))] }) }
fn create_form() -> Value { json!({ "title": "New Skill", "submit": "Create Skill", "fields": [{ "id": "name", "title": "Name", "placeholder": "release-notes" }, use_when(None), instr_field(None)] }) }
fn preview_form() -> Value { json!({ "title": "Add a Skill", "submit": "Preview", "fields": [{ "id": "from", "title": "GitHub link or folder", "placeholder": "https://github.com/owner/repository/tree/main/skill" }] }) }
fn remove_action(title: &str, builtin: bool) -> Value {
    let body = if builtin { "This built-in skill will be removed from all your computers. You can add it again from the Kuvo catalog." }
        else { "This skill will be removed from all your computers. You can restore its files from Recently Deleted for 30 days." };
    json!({ "id": "remove", "title": "Remove", "style": "destructive", "confirm": { "title": format!("Remove {title}?"), "button": "Remove", "body": body } })
}
fn plural(n: usize, w: &str) -> String { format!("{n} {w}{}", if n == 1 { "" } else { "s" }) }

fn build_area(id: &str, title: &str, items: Vec<Value>, query: &HashMap<String, String>, actions: Value) -> Value {
    let mut titles = Vec::new();
    for item in &items {
        let c = str_at(item, "category");
        if !titles.iter().any(|t| t == c) { titles.push(c.to_owned()); }
    }
    if let Some(mine) = match id { "skills" => Some(YOUR_SKILLS), "apps" => Some("Yours"), _ => None }
        && let Some(pos) = titles.iter().position(|t| t == mine)
    {
        let m = titles.remove(pos);
        if id == "skills" { titles.insert(0, m); } else { titles.push(m); }
    }
    let categories: Vec<Value> = titles.iter().map(|t| json!({ "title": t, "count": items.iter().filter(|i| opt(i, "category") == Some(t.as_str())).count() })).collect();
    let search = query.get("q").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    let (cat_f, orig_f, total) = (query.get("category").filter(|s| !s.is_empty()), query.get("origin").filter(|s| !s.is_empty()), items.len());
    let results: Vec<Value> = items.into_iter().filter(|i| {
        cat_f.is_none_or(|c| opt(i, "category") == Some(c.as_str()))
            && (id == "agents" || orig_f.is_none_or(|o| (o == "kuvo") == (opt(i, "origin") == Some("catalog"))))
            && (search.is_empty() || format!("{}\n{}", str_at(i, "title"), str_at(i, "summary")).to_ascii_lowercase().contains(&search))
    }).collect();
    let line = match id { "apps" => "Connect services once for all your computers.", "agents" => "Ready-made bots, with their instructions, skills and the apps they need.", "skills" => "Instructions that teach an agent a task.", _ => "" };
    let mut out = json!({
        "id": id, "title": title, "line": line, "count": total, "categories": categories, "items": results,
        "actions": actions, "empty": if search.is_empty() { "There’s nothing here yet." } else { "No items match your search." },
    });
    if id != "agents" { out["filters"] = json!([{ "id": "kuvo", "title": "Kuvo" }, { "id": "yours", "title": "Yours" }]); }
    out
}

fn random_uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!("{}-{}-{}-{}-{}", hex::encode(&b[0..4]), hex::encode(&b[4..6]), hex::encode(&b[6..8]), hex::encode(&b[8..10]), hex::encode(&b[10..16]))
}

fn parse_query(path: &str) -> HashMap<String, String> {
    path.split_once('?').map_or_else(HashMap::new, |(_, q)| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
}
fn parse_body(bytes: &[u8]) -> Value { serde_json::from_slice::<Value>(bytes).ok().filter(Value::is_object).unwrap_or_else(|| json!({})) }
fn reply<R: kuvo_protocol::JsonStatus<200> + kuvo_protocol::RouteSpec>(pair: (u16, Value)) -> Result<RouteReply<R>, Refusal> {
    if pair.0 == 200 { Ok(RouteReply::json_as_kept::<200>(pair.1)) } else { Err(pair_refusal(pair)) }
}
fn conn_pair(e: ConnectionError) -> (u16, Value) { no(e.status, &e.message) }
fn decode_id(raw: Option<&str>, folder: bool) -> Result<String, (u16, Value)> {
    let id = decode_uri_component(raw.unwrap_or("")).ok_or_else(|| no(400, "That item name isn’t valid."))?;
    if if folder { is_skill_folder(&id) } else { is_item_id(&id) } { Ok(id) } else { Err((404, json!({}))) }
}
fn builtin_available(skill: &BuiltinSkill) -> Value {
    json!({ "version": "built-in", "revision": sha256_hex(serde_json::to_string(skill).unwrap_or_default().as_bytes()), "files": [] })
}

macro_rules! reg {
    ($routes:expr, $svc:expr, $spec:ty, |$s:ident, $q:ident, $b:ident| $body:expr) => {{
        let s = $svc.clone();
        $routes.register::<$spec, _, _>(Access::DesktopOrPaired, move |call| {
            let ($s, $q, $b) = (s.clone(), parse_query(&call.request.path), parse_body(&call.request.body));
            async move { reply($body) }
        });
    }};
    ($routes:expr, $svc:expr, $spec:ty, id($folder:expr), |$s:ident, $id:ident, $q:ident, $b:ident| $body:expr) => {{
        let s = $svc.clone();
        $routes.register::<$spec, _, _>(Access::DesktopOrPaired, move |call| {
            let ($s, raw, $q, $b) = (s.clone(), call.param("id").map(str::to_owned), parse_query(&call.request.path), parse_body(&call.request.body));
            async move { reply(match decode_id(raw.as_deref(), $folder) { Err(e) => e, Ok($id) => $body }) }
        });
    }};
    ($routes:expr, $svc:expr, $spec:ty, app, |$s:ident, $app:ident, $q:ident, $b:ident| $body:expr) => {{
        reg!($routes, $svc, $spec, id(false), |$s, id, $q, $b| match $s.require_app(&id) { Err(e) => e, Ok($app) => $body });
    }};
}

pub fn register(routes: &mut Registry, svc: Arc<MarketplaceService>) {
    reg!(routes, svc, get_kuvo_marketplace::Spec, |s, q, _b| s.catalog_view(&q).await);
    reg!(routes, svc, post_kuvo_marketplace_refresh::Spec, |s, _q, _b| match s.catalog.refresh().await {
        Ok(refreshed) => (200, json!({ "refreshed": refreshed })),
        Err(_) => (200, json!({ "refreshed": false, "message": "The catalog couldn’t be refreshed. Showing the last available copy." })),
    });
    reg!(routes, svc, post_kuvo_marketplace_apps_custom::Spec, |s, _q, b| {
        let (title, url) = (str_at(&b, "title"), str_at(&b, "url"));
        let id = opt(&b, "id").map(str::to_owned).unwrap_or_else(|| skill_name(title));
        let app = InstalledApp { id: id.clone(), title: title.into(), category: "Yours".into(), summary: opt(&b, "summary").unwrap_or("Custom MCP server.").into(), auth: opt(&b, "auth").unwrap_or("open").into(), url: url.into(), origin: "custom".into() };
        if !is_endpoint(url) || !valid_installed_app(&app) { no(400, "Enter a name and an HTTPS MCP address without credentials or query parameters.") }
        else if s.catalog.catalog().apps.iter().any(|a| a.id == id) { no(409, "A catalog app already uses that name.") }
        else { let (st, mut v) = s.save("save-app", json!({ "app": app }), &[]).await; if let Some(m) = v.as_object_mut() { m.insert("id".into(), json!(id)); } (st, v) }
    });
    {
        let s = svc.clone();
        routes.register::<get_kuvo_marketplace_apps_id_logo::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let (s, raw) = (s.clone(), call.param("id").map(str::to_owned));
            async move {
                let id = decode_id(raw.as_deref(), false).map_err(pair_refusal)?;
                let logo = s.catalog.offered_apps().into_iter().find(|a| a.id == id).and_then(|a| a.logo).ok_or_else(|| pair_refusal((404, json!({}))))?;
                let bytes = STANDARD.decode(&logo).map_err(|_| pair_refusal((404, json!({}))))?;
                Ok(RouteReply::bytes::<200>("image/png", bytes))
            }
        });
    }
    reg!(routes, svc, get_kuvo_marketplace_apps_id::Spec, id(false), |s, id, _q, _b| match s.read_state() {
        Err(e) => e,
        Ok(st) => {
            let inst = st.apps.iter().find(|a| a.id == id);
            inst.cloned().or_else(|| s.catalog_app(&id)).map_or_else(|| no(404, MISSING), |a| (200, s.app_card(&a, inst.is_some(), true)))
        }
    });
    reg!(routes, svc, post_kuvo_marketplace_apps_id_add::Spec, id(false), |s, id, _q, _b| match s.read_state() {
        Err(e) => e,
        Ok(st) => match st.apps.into_iter().find(|a| a.id == id).or_else(|| s.catalog_app(&id)) {
            Some(a) => s.save("save-app", json!({ "app": a }), &[]).await,
            None => no(404, MISSING),
        },
    });
    reg!(routes, svc, delete_kuvo_marketplace_apps_id::Spec, id(false), |s, id, _q, _b| match s.read_state() {
        Err(e) => e,
        Ok(st) if !st.apps.iter().any(|a| a.id == id) && s.catalog_app(&id).is_none() => no(404, MISSING),
        Ok(_) => { let p = s.save("remove-app", json!({ "id": id }), &[]).await; if p.0 == 200 { s.connections.clean_removed_connections(); } p }
    });
    {
        let s = svc.clone();
        routes.register::<post_kuvo_marketplace_apps_id_connect::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let (s, raw, b) = (s.clone(), call.param("id").map(str::to_owned), parse_body(&call.request.body));
            async move {
                let id = decode_id(raw.as_deref(), false).map_err(pair_refusal)?;
                let app = s.require_app(&id).map_err(pair_refusal)?;
                if app.auth == "oauth" {
                    let started = s.connections.start_sign_in(&app).await.map_err(|e| pair_refusal(conn_pair(e)))?;
                    return Ok(RouteReply::json_as_kept::<202>(started));
                }
                if app.auth == "token" { s.connections.connect_token(&app, str_at(&b, "token")).map_err(|e| pair_refusal(conn_pair(e)))?; }
                Ok(RouteReply::json_as_kept::<200>(json!({ "connection": s.connections.connection_state(&app), "problems": s.connections.project_apps(None) })))
            }
        });
    }
    reg!(routes, svc, post_kuvo_marketplace_apps_id_disconnect::Spec, app, |s, app, _q, _b| match s.connections.disconnect(&app) {
        Err(e) => conn_pair(e),
        Ok(()) => (200, json!({ "connection": s.connections.connection_state(&app), "problems": s.connections.project_apps(None) })),
    });
    reg!(routes, svc, get_kuvo_marketplace_apps_id_status::Spec, app, |s, app, q, _b| s.connections.sign_in_status(&app, q.get("attempt").map(String::as_str).unwrap_or("")).map_or_else(conn_pair, |v| (200, v)));
    reg!(routes, svc, post_kuvo_marketplace_apps_id_cancel::Spec, app, |s, app, _q, b| s.connections.cancel_sign_in(&app, str_at(&b, "attempt")).map_or_else(conn_pair, |()| (200, json!({ "state": "cancelled" }))));
    reg!(routes, svc, post_kuvo_marketplace_apps_id_finish::Spec, app, |s, app, _q, b| match s.connections.finish_sign_in(&app, str_at(&b, "attempt"), str_at(&b, "address")).await {
        Err(e) => conn_pair(e),
        Ok(()) => (200, json!({ "connection": s.connections.connection_state(&app), "problems": s.connections.project_apps(None) })),
    });
    reg!(routes, svc, get_kuvo_marketplace_agents_id::Spec, id(false), |s, id, _q, _b| match s.catalog.offered_agents().into_iter().find(|a| a.id == id) {
        None => no(404, MISSING),
        Some(agent) => {
            let mut val = serde_json::to_value(&agent).unwrap_or_default();
            let mut labels = json!({
                "computer": "Runs on", "model": "Account and model", "routine": "Enable scheduled routines",
                "name": "Name", "apps": "Needed apps", "routines": "Routines", "off": "Routines stay off until you turn them on.",
            });
            if !agent.skills.is_empty() { labels["skills"] = json!(format!("Includes {}: {}", plural(agent.skills.len(), "skill"), agent.skills.join(", "))); }
            val["bots"] = json!(s.all_agent_bots().remove(&id).unwrap_or_default());
            val["actions"] = json!([{ "id": "add", "title": "Add Agent" }]);
            val["computers"] = Value::Array(s.computers_json());
            val["labels"] = labels;
            (200, val)
        }
    });
    reg!(routes, svc, get_kuvo_marketplace_agents_id_options::Spec, id(false), |s, id, q, _b| match s.catalog.offered_agents().into_iter().find(|a| a.id == id) {
        None => no(404, MISSING),
        Some(agent) => {
            let comps = s.peers.computers(&s.computer_name);
            let Some(comp) = comps.iter().find(|c| c.key == q.get("computer").map(String::as_str).unwrap_or("")) else { return reply(no(400, "Choose a computer in your setup.")); };
            let (st, mut val) = if comp.here { (200, bot_options(&agent, &s.catalog, &s.installations, &s.connections)) }
                else { s.peers.forward(&comp.key, PeerAct { method: "GET".into(), item: format!("options/{id}"), body: json!({}) }).await };
            if st == 200 && let Some(missing) = val.get_mut("missing").and_then(Value::as_array_mut) {
                let n = missing.len();
                for a in missing.iter_mut() {
                    a["connect"] = Value::Bool(comp.here);
                    a["words"] = json!(if comp.here { "Not connected".into() } else { format!("Connect it on {} first", comp.name) });
                }
                if n > 0 { val["blocked"] = json!(format!("{} still to connect", plural(n, "app"))); }
            }
            (st, val)
        }
    });
    {
        let s = svc.clone();
        routes.register::<post_kuvo_marketplace_agents_id_add::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let (s, raw, b) = (s.clone(), call.param("id").map(str::to_owned), parse_body(&call.request.body));
            async move {
                let id = decode_id(raw.as_deref(), false).map_err(pair_refusal)?;
                let agent = s.catalog.offered_agents().into_iter().find(|a| a.id == id).ok_or_else(|| pair_refusal(no(404, MISSING)))?;
                let comps = s.peers.computers(&s.computer_name);
                let comp = comps.iter().find(|c| c.key == str_at(&b, "computer")).ok_or_else(|| pair_refusal(no(400, "Choose a computer in your setup.")))?;
                if opt(&b, "version") != Some(agent.version.as_str()) { return Err(pair_refusal(no(409, "Review the current version before adding this agent."))); }
                let saved = s.save("agent-skills", json!({ "id": id, "version": agent.version }), &[]).await;
                if saved.0 != 200 { return Err(pair_refusal(saved)); }
                let (st, val) = if comp.here { s.add_agent_local(&agent, &b) } else { s.peers.forward(&comp.key, PeerAct { method: "POST".into(), item: format!("add-agent/{id}"), body: b }).await };
                if st == 201 { Ok(RouteReply::json_as_kept::<201>(val)) } else { Err(pair_refusal((st, val))) }
            }
        });
    }
    reg!(routes, svc, post_kuvo_marketplace_skills_preview::Spec, |s, _q, b| {
        let from = opt(&b, "from").map(str::trim);
        let folder = if let Some(f) = from && (f.starts_with('/') || f == "~" || f.starts_with("~/")) {
            Some(f.strip_prefix("~/").map_or_else(|| if f == "~" { s.account_home.clone() } else { PathBuf::from(f) }, |r| s.account_home.join(r)))
        } else { opt(&b, "folder").map(PathBuf::from) };
        let res: Result<InstalledSkill, SkillFailure> = if let Some(dir) = folder {
            if !dir.exists() { Err(SkillFailure::new(400, "Choose a folder on this computer that contains SKILL.md.")) }
            else { local_skill(&dir).map(|p| InstalledSkill { name: p.name, files: p.files, source: p.source, origin: "folder".into(), version: None }) }
        } else {
            let given = from.map_or_else(|| b.clone(), |link| json!({ "link": link }));
            s.skill_library.github_skill(&given, false).await.map(|p| InstalledSkill { name: p.name, files: p.files, source: p.source, origin: "github".into(), version: None })
        };
        match res { Ok(sk) => s.stage_preview(sk), Err(e) => no(e.status, &e.message) }
    });
    reg!(routes, svc, post_kuvo_marketplace_skills_create::Spec, |s, _q, b| match written_skill(&b, None, "") {
        None => no(400, "Fill in the name, description and instructions."),
        Some(sk) => s.save_skill_with_id(sk, b.get("was")).await,
    });
    reg!(routes, svc, post_kuvo_marketplace_skills_install::Spec, |s, _q, b| match s.find_preview(opt(&b, "preview")) {
        None => no(400, "Preview the skill again first."),
        Some(sk) => s.save_skill_with_id(sk, b.get("was")).await,
    });
    reg!(routes, svc, post_kuvo_marketplace_skills_restore::Spec, |s, _q, b| s.skill_restore(&b).await);
    reg!(routes, svc, post_kuvo_marketplace_skills_places::Spec, |s, _q, b| match opt(&b, "place").map(str::trim).filter(|p| !p.is_empty()) {
        None => no(400, "Give a folder or catalog address."),
        Some(place) => s.edit_places(|p| p.push(json!(place))),
    });
    reg!(routes, svc, delete_kuvo_marketplace_skills_places::Spec, |s, _q, b| match b.get("index").and_then(Value::as_u64) {
        None => no(400, "Choose a place to remove."),
        Some(idx) => s.edit_places(|p| if (idx as usize) < p.len() { p.remove(idx as usize); }),
    });
    reg!(routes, svc, post_kuvo_marketplace_skills_id_enabled::Spec, id(true), |s, id, _q, b| match b.get("on").and_then(Value::as_bool) {
        None => no(400, r#"Send {"on": true} or false."#),
        Some(on) => match s.config.read_settings() {
            SettingsRead::Problem(p) => no(409, &p),
            SettingsRead::Ok(m) => {
                let before = m.get("permissions").cloned();
                let mut kept: Vec<Value> = before.as_ref().and_then(Value::as_array).cloned().unwrap_or_default().into_iter()
                    .filter(|r| !(opt(r, "action") == Some("skill") && opt(r, "resource") == Some(id.as_str()) && opt(r, "effect").is_some_and(|e| !matches!(e, "allow" | "ask"))))
                    .collect();
                if !on { kept.push(json!({ "action": "skill", "resource": id, "effect": "deny" })); }
                match s.config.change_settings(&json!([{ "path": ["permissions"], "value": kept, "was": before.unwrap_or(Value::Null) }])) {
                    SettingsChanged::Ok(_) => (200, json!({ "enabled": on })),
                    SettingsChanged::Err { status, message, .. } => no(status, &message),
                }
            }
        },
    });
    reg!(routes, svc, post_kuvo_marketplace_skills_id_update_preview::Spec, id(true), |s, id, _q, b| match s.read_state() {
        Err(e) => e,
        Ok(st) => match st.skills.into_iter().find(|x| x.name == id).and_then(|x| x.source) {
            None => no(404, MISSING),
            Some(src) => match s.skill_library.github_skill(&json!({ "repository": src.repository, "path": src.path, "ref": opt(&b, "ref").or(src.r#ref.as_deref()) }), false).await {
                Ok(p) => s.stage_preview(InstalledSkill { name: p.name, files: p.files, source: p.source, origin: "github".into(), version: None }),
                Err(e) => no(e.status, &e.message),
            },
        },
    });
    reg!(routes, svc, post_kuvo_marketplace_skills_id_add::Spec, id(true), |s, id, _q, b| match s.read_state() {
        Err(e) => e,
        Ok(st) => {
            if let Some(listed) = s.catalog.catalog().skills.into_iter().find(|x| x.id == id) {
                let offered = MarketplaceService::offer_skill(&listed);
                if opt(&b, "version") != Some(listed.version.as_str()) || opt(&b, "revision") != Some(skill_revision(&offered).as_str()) {
                    no(409, "Review the current version before installing it.")
                } else if s.files_on_this_box(&id) { no(409, "A local skill already uses that name.") }
                else { s.save("save-skill", json!({ "skill": offered, "was": b.get("was") }), &[]).await }
            } else if let Some(rem) = removal_for(&st, &id) && let Some(bi) = &rem.builtin {
                let avail = builtin_available(bi);
                if b.get("version") != avail.get("version") || b.get("revision") != avail.get("revision") { no(409, "Review the skill before adding it.") }
                else { s.save("restore-skill", json!({ "id": id, "key": rem.key }), &[]).await }
            } else { no(404, MISSING) }
        }
    });
    reg!(routes, svc, get_kuvo_marketplace_skills_id::Spec, id(true), |s, id, _q, _b| s.skill_get(&id).await);
    reg!(routes, svc, patch_kuvo_marketplace_skills_id::Spec, id(true), |s, id, _q, b| s.skill_patch(&id, &b).await);
    reg!(routes, svc, delete_kuvo_marketplace_skills_id::Spec, id(true), |s, id, _q, b| s.skill_delete(&id, &b).await);
}
