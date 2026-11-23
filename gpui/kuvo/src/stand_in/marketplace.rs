//! The Marketplace as a stand-in box answers it (`kuvo/marketplace`, PROTOCOL.md "Marketplace"):
//! the real catalog the box ships, with each app's own logo, and what's been added to this made-up
//! setup: apps connected, a custom server, skills from the catalog, GitHub and this computer's own
//! folder, and the bots made from agents. Nothing real is reached: a GitHub link previews a made-up
//! skill, and a sign-in finishes when a test says so.

use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use serde_json::{Value, json};

use super::bots::Bots;

/// The catalog the box ships (`catalog/index.json`), logos and all.
const CATALOG: &str = include_str!("../../../../catalog/index.json");

/// What the setup has added, for a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sample {
    /// Nothing added yet: the catalog alone.
    #[default]
    Fresh,
    /// A setup in use: apps connected, a custom server, skills of every kind, a second computer.
    InUse,
}

pub struct Market {
    catalog: Value,
    /// Apps added, by id: their card fields, and whether connected on this computer.
    apps: Vec<(Value, bool)>,
    /// Skills installed or yours: `{id, title, summary, category, origin, files, version?, source?}`.
    skills: Vec<Value>,
    /// Skills the engine finds in other folders.
    found: Vec<Value>,
    disabled: HashSet<String>,
    deleted: Vec<Value>,
    places: Vec<String>,
    previews: HashMap<String, Value>,
    /// Sign-ins started (attempt → app), and those finished.
    attempts: HashMap<String, String>,
    finished: HashSet<String>,
    /// Bots made from each agent.
    made: HashMap<String, usize>,
    /// A second computer in the setup.
    pub other: Option<(String, String)>,
    next: usize,
}

impl Default for Market {
    fn default() -> Self {
        let catalog: Value = serde_json::from_str(CATALOG).expect("the catalog reads");
        Market {
            catalog,
            apps: Vec::new(),
            skills: Vec::new(),
            found: Vec::new(),
            disabled: HashSet::new(),
            deleted: Vec::new(),
            places: Vec::new(),
            previews: HashMap::new(),
            attempts: HashMap::new(),
            finished: HashSet::new(),
            made: HashMap::new(),
            other: None,
            next: 1,
        }
    }
}

fn file(path: &str, text: &str) -> Value {
    json!({ "path": path, "data": base64::engine::general_purpose::STANDARD.encode(text), "sha256": format!("{:064x}", text.len()) })
}

fn use_it_when() -> Value {
    json!({ "id": "description", "title": "Use it when", "kind": "lines", "placeholder": "Writing release notes from the commits since the last tag", "line": "The agent reads this to decide when the skill is needed, and loads the instructions only then." })
}

fn instructions() -> Value {
    json!({ "id": "instructions", "title": "Instructions", "kind": "lines", "mono": true, "line": "Markdown. Mention any other files in the skill so the agent knows to use them." })
}

fn create_form() -> Value {
    json!({ "title": "New Skill", "submit": "Create Skill", "fields": [{ "id": "name", "title": "Name", "placeholder": "release-notes" }, use_it_when(), instructions()] })
}

fn preview_form() -> Value {
    json!({ "title": "Add a Skill", "submit": "Preview", "fields": [{ "id": "from", "title": "GitHub link or folder", "placeholder": "https://github.com/owner/repository/tree/main/skill" }] })
}

fn edit_form(title: &str, description: &str, body: &str) -> Value {
    let mut when = use_it_when();
    when["value"] = json!(description);
    let mut what = instructions();
    what["value"] = json!(body);
    json!({ "title": format!("Edit {title}"), "submit": "Save", "fields": [when, what] })
}

fn skill_md(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n")
}

const PR_CHECKLIST: &str = "# PR checklist\n\nWalk the diff once for each of these, and say what you found:\n\n1. Edge cases: empty input, one item, very many.\n2. Errors: what fails, and what the user sees then.\n3. Tests: what changed that no test covers.\n4. Regressions: what used to work that this could break.\n\nUse `examples/review-template.md` for the write-up.";

impl Market {
    pub fn sample(&mut self, sample: Sample) {
        *self = Market::default();
        if sample == Sample::Fresh {
            return;
        }
        for (id, connected) in [("github", false), ("notion", true), ("cloudflare-docs", true), ("linear", false), ("sentry", true)] {
            if let Some(app) = self.catalog_app(id) {
                self.apps.push((app, connected));
            }
        }
        self.apps.push((json!({ "id": "internal-deploy", "title": "internal-deploy", "category": "Yours", "summary": "Staging builds and release checks.", "origin": "custom", "auth": "oauth", "url": "https://deploy.example.com/mcp" }), true));
        if let Some(skill) = self.catalog_skill("review-changes") {
            self.skills.push(skill);
        }
        self.skills.push(json!({
            "id": "pr-checklist", "title": "pr-checklist", "summary": "Walk a diff for edge cases, error handling, missing tests and regressions.",
            "category": "Your skills", "origin": "github", "source": { "repository": "acme/agent-skills", "path": "pr-checklist", "commit": "3f2c9a1d0b7e" },
            "files": [file("SKILL.md", &skill_md("pr-checklist", "Walk a diff for edge cases, error handling, missing tests and regressions.", PR_CHECKLIST))],
        }));
        self.skills.push(json!({
            "id": "release", "title": "release", "summary": "Write release notes from the commits since the last tag.", "category": "Your skills", "origin": "local",
            "files": [file("SKILL.md", &skill_md("release", "Write release notes from the commits since the last tag.", "List the commits since the last tag, group them by what users notice, and write one plain line for each."))],
        }));
        self.found.push(json!({ "id": "pdf", "title": "pdf", "summary": "Read, fill and merge PDF files.", "category": "Your skills", "origin": "found", "location": "~/.claude/skills/pdf" }));
        self.disabled.insert("pdf".into());
        self.deleted.push(json!({ "id": "1794600000000-meeting-notes", "title": "meeting-notes", "summary": "Turn a meeting transcript into decisions and next steps." }));
        self.made.insert("coding-reviewer".into(), 1);
        self.other = Some(("mini-key".into(), "Studio Mini".into()));
    }

    /// Every sign-in started finishes, as a browser's would.
    pub fn finish_sign_ins(&mut self) {
        let attempts: Vec<(String, String)> = self.attempts.drain().collect();
        for (attempt, app) in attempts {
            self.finished.insert(attempt);
            if let Some(added) = self.apps.iter_mut().find(|(each, _)| each["id"] == json!(app)) {
                added.1 = true;
            }
        }
    }

    fn list(&self, kind: &str) -> Vec<Value> {
        self.catalog[kind].as_array().cloned().unwrap_or_default()
    }

    fn offered_apps(&self) -> Vec<Value> {
        self.list("apps").into_iter().filter(|app| app["auth"] != json!("broker")).collect()
    }

    fn catalog_app(&self, id: &str) -> Option<Value> {
        self.offered_apps().into_iter().find(|app| app["id"] == json!(id)).map(|app| {
            json!({ "id": app["id"], "title": app["title"], "category": app["category"], "summary": app["summary"], "origin": "catalog", "auth": app["auth"], "url": app["url"] })
        })
    }

    fn catalog_skill(&self, id: &str) -> Option<Value> {
        self.list("skills").into_iter().find(|skill| skill["id"] == json!(id)).map(|mut skill| {
            skill["origin"] = json!("catalog");
            skill
        })
    }

    fn agents(&self) -> Vec<Value> {
        let offered: Vec<String> = self.offered_apps().iter().filter_map(|app| app["id"].as_str().map(str::to_owned)).collect();
        self.list("agents").into_iter().filter(|agent| agent["apps"].as_array().is_none_or(|apps| apps.iter().all(|id| id.as_str().is_some_and(|id| offered.iter().any(|each| each == id))))).collect()
    }

    fn connected(&self, id: &str) -> bool {
        self.apps.iter().any(|(app, connected)| app["id"] == json!(id) && (*connected || app["auth"] == json!("open")))
    }

    fn app_card(&self, app: &Value, installed: bool, connected: bool, details: bool) -> Value {
        let open = app["auth"] == json!("open");
        let ready = installed && (connected || open);
        let mut card = json!({
            "id": app["id"], "title": app["title"], "category": app["category"], "summary": app["summary"], "origin": app["origin"],
            "installed": installed, "connection": if !installed { Value::Null } else if ready { json!("ready") } else { json!("sign-in") },
        });
        if app["origin"] == json!("catalog") {
            card["logo"] = json!(format!("kuvo/marketplace/apps/{}/logo", app["id"].as_str().unwrap_or_default()));
        }
        if ready {
            card["status"] = json!({ "words": if open { "Added" } else { "Connected" }, "tone": "good" });
        }
        card["actions"] = if installed {
            let mut actions = Vec::new();
            if !open {
                actions.push(json!({ "id": "connect", "title": if ready { "Reconnect" } else { "Connect" } }));
                if ready {
                    actions.push(json!({ "id": "disconnect", "title": "Disconnect on This Computer" }));
                }
            }
            actions.push(json!({ "id": "remove", "title": "Remove from All Computers", "style": "destructive", "confirm": {
                "title": format!("Remove {}?", app["title"].as_str().unwrap_or_default()),
                "body": "It's removed from all your computers, with each computer's sign-in. Agents can't use it until you add it again.", "button": "Remove" } }));
            json!(actions)
        } else {
            json!([{ "id": "add", "title": if open { "Add" } else { "Connect" } }])
        };
        if details {
            card["url"] = app["url"].clone();
            card["auth"] = app["auth"].clone();
            if app["auth"] == json!("token") {
                let mut form = json!({ "title": format!("Connect {}", app["title"].as_str().unwrap_or_default()), "submit": "Connect", "fields": [{ "id": "token", "title": "Personal access token", "kind": "password" }] });
                if app["id"] == json!("github") {
                    form["help"] = json!("https://github.com/settings/personal-access-tokens/new");
                }
                card["form"] = form;
            }
        }
        card
    }

    fn computers(&self, name: &str) -> Value {
        let mut list = vec![json!({ "key": "here-key", "name": name, "here": true, "available": true })];
        if let Some((key, name)) = &self.other {
            list.push(json!({ "key": key, "name": name, "here": false, "available": true }));
        }
        json!(list)
    }

    fn here_skills(&self, name: &str) -> Value {
        json!({
            "computer": name, "switchTitle": format!("Use on {name}"),
            "deleted": { "title": "Recently Deleted", "note": "Deleted skills are kept for 30 days, then removed for good.", "items": self.deleted, "action": { "id": "restore", "title": "Restore" } },
            "places": {
                "title": "Other Places", "note": "A folder, or the address of a skill catalog with an index.json, where agents also find skills.",
                "items": self.places.iter().enumerate().map(|(ix, place)| json!({ "id": ix.to_string(), "title": place })).collect::<Vec<_>>(),
                "add": { "id": "add", "title": "Add Place", "form": { "title": "Add a Place", "submit": "Add", "fields": [{ "id": "place", "title": "A folder, or the address of a skill catalog" }] } },
                "remove": { "id": "remove", "title": "Remove" },
            },
        })
    }

    fn browse(&self, query: &dyn Fn(&str) -> Option<String>, name: &str) -> Value {
        let mut apps: Vec<Value> = self.apps.iter().map(|(app, connected)| self.app_card(app, true, *connected, false)).collect();
        for app in self.offered_apps() {
            let id = app["id"].as_str().unwrap_or_default();
            if !self.apps.iter().any(|(each, _)| each["id"] == app["id"]) {
                apps.push(self.app_card(&self.catalog_app(id).unwrap_or_default(), false, false, false));
            }
        }
        let agents: Vec<Value> = self
            .agents()
            .into_iter()
            .map(|mut agent| {
                let id = agent["id"].as_str().unwrap_or_default().to_owned();
                agent.as_object_mut().map(|agent| agent.remove("instructions"));
                let count = self.made.get(&id).copied().unwrap_or(0);
                let mut detail: Vec<String> = agent["apps"].as_array().into_iter().flatten().filter_map(|id| self.offered_apps().into_iter().find(|app| &app["id"] == id).and_then(|app| app["title"].as_str().map(str::to_owned))).collect();
                let skills = agent["skills"].as_array().map_or(0, Vec::len);
                detail.push(format!("{skills} skill{}", if skills == 1 { "" } else { "s" }));
                if count > 0 {
                    detail.push(format!("{count} bot{} added", if count == 1 { "" } else { "s" }));
                }
                agent["botCount"] = json!(count);
                agent["detail"] = json!(detail.join(" · "));
                agent["actions"] = json!([{ "id": "add", "title": "Add" }]);
                agent
            })
            .collect();
        let installed: HashSet<String> = self.skills.iter().filter_map(|skill| skill["id"].as_str().map(str::to_owned)).collect();
        let mut skills: Vec<Value> = self
            .list("skills")
            .into_iter()
            .map(|mut skill| {
                let id = skill["id"].as_str().unwrap_or_default().to_owned();
                skill.as_object_mut().map(|skill| skill.remove("files"));
                skill["origin"] = json!("catalog");
                skill["installed"] = json!(installed.contains(&id));
                if installed.contains(&id) {
                    skill["enabled"] = json!(!self.disabled.contains(&id));
                } else {
                    skill["actions"] = json!([{ "id": "add", "title": "Install" }]);
                }
                skill
            })
            .collect();
        for skill in self.skills.iter().filter(|skill| skill["origin"] != json!("catalog")).chain(&self.found) {
            let mut shown = skill.clone();
            shown.as_object_mut().map(|skill| skill.remove("files"));
            shown["installed"] = json!(true);
            shown["enabled"] = json!(!self.disabled.contains(skill["id"].as_str().unwrap_or_default()));
            skills.push(shown);
        }
        let area = |id: &str, title: &str, line: &str, items: Vec<Value>, actions: Value| -> Value {
            let mut categories: Vec<(String, usize)> = Vec::new();
            for item in &items {
                let category = item["category"].as_str().unwrap_or_default().to_owned();
                match categories.iter_mut().find(|(each, _)| *each == category) {
                    Some((_, count)) => *count += 1,
                    None => categories.push((category, 1)),
                }
            }
            // In the order shown, as the box puts them: your own skills first, your own apps last.
            let mine = match id { "skills" => Some("Your skills"), "apps" => Some("Yours"), _ => None };
            if let Some(mine) = mine
                && let Some(at) = categories.iter().position(|(title, _)| title == mine)
            {
                let yours = categories.remove(at);
                if id == "skills" { categories.insert(0, yours) } else { categories.push(yours) }
            }
            let words = query("q").unwrap_or_default().trim().to_lowercase();
            let category = query("category");
            let origin = query("origin");
            let shown: Vec<Value> = items
                .iter()
                .filter(|item| category.as_deref().is_none_or(|category| item["category"] == json!(category)))
                .filter(|item| id == "agents" || origin.as_deref().is_none_or(|origin| (origin == "kuvo") == (item["origin"] == json!("catalog"))))
                .filter(|item| words.is_empty() || format!("{}\n{}", item["title"].as_str().unwrap_or_default(), item["summary"].as_str().unwrap_or_default()).to_lowercase().contains(&words))
                .cloned()
                .collect();
            let mut area = json!({
                "id": id, "title": title, "line": line, "count": items.len(),
                "categories": categories.into_iter().map(|(title, count)| json!({ "title": title, "count": count })).collect::<Vec<_>>(),
                "items": shown, "actions": actions,
                "empty": if words.is_empty() { "There’s nothing here yet." } else { "No items match your search." },
            });
            if id != "agents" {
                area["filters"] = json!([{ "id": "kuvo", "title": "Kuvo" }, { "id": "yours", "title": "Yours" }]);
            }
            area
        };
        let custom = json!([{ "id": "custom", "title": "Add Custom", "form": {
            "title": "Add Custom App", "submit": "Add",
            "fields": [{ "id": "title", "title": "Name" }, { "id": "url", "title": "MCP server address", "placeholder": "https://example.com/mcp" },
                { "id": "auth", "title": "Sign-in", "choices": [{ "id": "open", "title": "No sign-in" }, { "id": "token", "title": "Personal access token" }, { "id": "oauth", "title": "Sign in with the service" }] }],
        } }]);
        let mut skills_area = area("skills", "Skills", "Instructions that teach an agent a task.", skills, json!([{ "id": "preview", "title": "Add from Link or Folder", "form": preview_form() }, { "id": "create", "title": "New Skill", "form": create_form() }]));
        skills_area["here"] = self.here_skills(name);
        json!({
            "title": "Marketplace", "computers": self.computers(name), "searchPlaceholder": "Search, or paste a GitHub link",
            "areas": [
                area("apps", "Apps", "Connect services once for all your computers.", apps, custom),
                area("agents", "Agents", "Ready-made bots, with their instructions, skills and the apps they need.", agents, json!([])),
                skills_area,
            ],
        })
    }

    fn skill_details(&self, id: &str) -> Option<Value> {
        let mine = self.skills.iter().find(|skill| skill["id"] == json!(id)).cloned();
        let listed = self.catalog_skill(id);
        let info = mine.clone().or_else(|| listed.clone())?;
        let mut shown = info.clone();
        shown["installed"] = json!(mine.is_some());
        shown["revision"] = json!(format!("rev-{id}"));
        if let Some(listed) = &listed {
            shown["available"] = json!({ "version": listed["version"], "revision": format!("rev-{id}") });
        }
        shown["actions"] = match &mine {
            None => json!([{ "id": "add", "title": "Install Skill" }]),
            Some(skill) => {
                let mut actions = vec![json!({ "id": "remove", "title": "Remove", "style": "destructive", "confirm": {
                    "title": format!("Remove {}?", skill["title"].as_str().unwrap_or(id)), "body": "It's removed from all your computers. You can install it again later.", "button": "Remove" } })];
                if skill.get("source").is_some_and(Value::is_object) {
                    actions.push(json!({ "id": "update-preview", "title": "Check for Update" }));
                } else if skill["origin"] != json!("catalog") {
                    let text = skill["files"][0]["data"].as_str().and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok()).and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_default();
                    let body = text.splitn(3, "---").nth(2).unwrap_or_default().trim().to_owned();
                    actions.push(json!({ "id": "edit", "title": "Edit", "form": edit_form(skill["title"].as_str().unwrap_or(id), skill["summary"].as_str().unwrap_or_default(), &body) }));
                }
                if skill["origin"] == json!("local") {
                    actions[0]["title"] = json!("Delete");
                    actions[0]["confirm"] = json!({ "title": format!("Delete {id}?"), "body": "Its folder, with anything else in it, is kept for 30 days in Recently Deleted, where you can restore it.", "button": "Delete" });
                }
                json!(actions)
            }
        };
        Some(shown)
    }

    fn preview(&mut self, body: &Value) -> (u16, Value) {
        let from = body["from"].as_str().map(str::trim).unwrap_or_default();
        let folder = (from.starts_with('/') || from.starts_with('~')).then_some(from);
        let (name, source) = if let Some(folder) = folder {
            (folder.trim_end_matches('/').rsplit('/').next().unwrap_or("skill").to_owned(), None)
        } else if let Some(link) = body["link"].as_str().or(Some(from).filter(|from| !from.is_empty())) {
            let link = link.trim();
            let Some(rest) = link.strip_prefix("https://github.com/") else {
                return (400, json!({ "message": "Paste a GitHub link to a skill’s folder." }));
            };
            let parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
            if parts.len() < 2 {
                return (400, json!({ "message": "Paste a GitHub link to a skill’s folder." }));
            }
            let name = parts.last().copied().unwrap_or("skill").to_owned();
            (name, Some(json!({ "repository": format!("{}/{}", parts[0], parts[1]), "path": parts.get(4..).map(|rest| rest.join("/")).unwrap_or_default(), "commit": "3f2c9a1d0b7e5c44" })))
        } else if let Some(folder) = body["folder"].as_str() {
            (folder.trim_end_matches('/').rsplit('/').next().unwrap_or("skill").to_owned(), None)
        } else {
            return (400, json!({ "message": "Paste a GitHub link to a skill’s folder." }));
        };
        let description = "Walk a diff for edge cases, error handling, missing tests and regressions.";
        let files = vec![
            file("SKILL.md", &skill_md(&name, description, PR_CHECKLIST)),
            file("examples/review-template.md", "## Review\n\n- Edge cases:\n- Errors:\n- Tests:\n- Regressions:\n"),
        ];
        let id = format!("{:064x}", self.next);
        self.next += 1;
        let mut skill = json!({ "id": name, "name": name, "description": description, "origin": if source.is_some() { "github" } else { "folder" }, "installed": false });
        if let Some(source) = &source {
            skill["source"] = source.clone();
        }
        self.previews.insert(id.clone(), json!({ "skill": skill, "files": files }));
        (200, json!({ "preview": id, "skill": skill, "installed": false, "files": files, "action": { "id": "install", "title": "Install Skill" } }))
    }

    /// Its answer to `method route` (a status, a type and the bytes), or none where it isn't the
    /// Marketplace's. `bots`: where Add makes a bot.
    pub fn answer(&mut self, method: &str, route: &str, query: &dyn Fn(&str) -> Option<String>, body: &Value, name: &str, bots: &mut Bots, now: f64) -> Option<(u16, &'static str, Vec<u8>)> {
        let rest = route.strip_prefix("kuvo/marketplace")?;
        let parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
        let json_answer = |status: u16, value: Value| Some((status, "application/json", value.to_string().into_bytes()));
        let missing = || json_answer(404, json!({ "message": "There’s no Marketplace item here." }));
        match (method, parts.as_slice()) {
            ("GET", []) => json_answer(200, self.browse(query, name)),
            ("GET", ["apps", id, "logo"]) => {
                let logo = self.offered_apps().into_iter().find(|app| app["id"] == json!(id)).and_then(|app| app["logo"].as_str().map(str::to_owned))?;
                Some((200, "image/png", base64::engine::general_purpose::STANDARD.decode(logo).ok()?))
            }
            ("POST", ["apps", "custom"]) => {
                let title = body["title"].as_str().unwrap_or_default().trim().to_owned();
                let url = body["url"].as_str().unwrap_or_default().trim().to_owned();
                if title.is_empty() || !url.starts_with("https://") || url.contains('?') {
                    return json_answer(400, json!({ "message": "Enter a name and an HTTPS MCP address without credentials or query parameters." }));
                }
                let id: String = title.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|word| !word.is_empty()).collect::<Vec<_>>().join("-");
                let auth = body["auth"].as_str().unwrap_or("open").to_owned();
                self.apps.push((json!({ "id": id, "title": title, "category": "Yours", "summary": "Custom MCP server.", "origin": "custom", "auth": auth, "url": url }), false));
                json_answer(200, json!({ "id": id, "problems": [] }))
            }
            ("GET", ["apps", id]) => match self.apps.iter().find(|(app, _)| app["id"] == json!(id)) {
                Some((app, connected)) => json_answer(200, self.app_card(app, true, *connected, true)),
                None => match self.catalog_app(id) {
                    Some(app) => json_answer(200, self.app_card(&app, false, false, true)),
                    None => missing(),
                },
            },
            ("POST", ["apps", id, "add"]) => {
                let app = self.catalog_app(id)?;
                if !self.apps.iter().any(|(each, _)| each["id"] == json!(id)) {
                    self.apps.push((app, false));
                }
                json_answer(200, json!({ "installed": true, "problems": [] }))
            }
            ("DELETE", ["apps", id]) => {
                self.apps.retain(|(app, _)| app["id"] != json!(id));
                json_answer(200, json!({ "installed": false, "problems": [] }))
            }
            ("POST", ["apps", id, action]) => {
                let Some(at) = self.apps.iter().position(|(app, _)| app["id"] == json!(id)) else {
                    return json_answer(409, json!({ "message": "Add the app first." }));
                };
                let auth = self.apps[at].0["auth"].as_str().unwrap_or("open").to_owned();
                match *action {
                    "connect" if auth == "token" => {
                        if body["token"].as_str().is_none_or(|token| token.len() < 4) {
                            return json_answer(400, json!({ "message": "Enter the personal access token issued by the service." }));
                        }
                        self.apps[at].1 = true;
                        json_answer(200, json!({ "connection": "ready", "problems": [] }))
                    }
                    "connect" if auth == "oauth" => {
                        let attempt = format!("attempt-{}", self.next);
                        self.next += 1;
                        self.attempts.insert(attempt.clone(), (*id).to_owned());
                        let title = self.apps[at].0["title"].as_str().unwrap_or_default().to_owned();
                        json_answer(202, json!({ "attempt": attempt, "opens": format!("https://auth.example.com/{id}/authorize?state={attempt}"), "title": format!("Sign in to {title}"), "callbackLabel": "Paste the address the sign-in page ends on", "submit": "Finish Sign-in" }))
                    }
                    "connect" => json_answer(200, json!({ "connection": "ready", "problems": [] })),
                    "finish" => {
                        let attempt = body["attempt"].as_str().unwrap_or_default().to_owned();
                        if self.attempts.remove(&attempt).is_none() {
                            return json_answer(404, json!({ "message": "That sign-in has ended. Start again." }));
                        }
                        self.finished.insert(attempt);
                        self.apps[at].1 = true;
                        json_answer(200, json!({ "connection": "ready", "problems": [] }))
                    }
                    "cancel" => {
                        self.attempts.remove(body["attempt"].as_str().unwrap_or_default());
                        json_answer(200, json!({ "state": "cancelled" }))
                    }
                    "disconnect" => {
                        self.apps[at].1 = false;
                        json_answer(200, json!({ "connection": "sign-in", "problems": [] }))
                    }
                    _ => missing(),
                }
            }
            ("GET", ["apps", _, "status"]) => {
                let attempt = query("attempt").unwrap_or_default();
                json_answer(200, json!({ "state": if self.finished.contains(&attempt) { "connected" } else { "waiting" } }))
            }
            ("GET", ["agents", id]) => {
                let agent = self.agents().into_iter().find(|agent| agent["id"] == json!(id))?;
                let mut shown = agent.clone();
                shown.as_object_mut().map(|agent| agent.remove("instructions"));
                let skills: Vec<String> = agent["skills"].as_array().into_iter().flatten().filter_map(|skill| skill.as_str().map(str::to_owned)).collect();
                shown["bots"] = json!([]);
                shown["actions"] = json!([{ "id": "add", "title": "Add Agent" }]);
                shown["computers"] = self.computers(name);
                shown["labels"] = json!({
                    "computer": "Runs on", "model": "Account and model", "routine": "Enable scheduled routines", "name": "Name", "apps": "Needed apps", "routines": "Routines",
                    "skills": if skills.is_empty() { Value::Null } else { json!(format!("Includes {} skill{}: {}", skills.len(), if skills.len() == 1 { "" } else { "s" }, skills.join(", "))) },
                    "off": "Routines stay off until you turn them on.",
                });
                json_answer(200, shown)
            }
            ("GET", ["agents", id, "options"]) => {
                let agent = self.agents().into_iter().find(|agent| agent["id"] == json!(id))?;
                let computer = query("computer").unwrap_or_default();
                let here = computer == "here-key";
                let other = self.other.as_ref().filter(|(key, _)| *key == computer).map(|(_, name)| name.clone());
                if !here && other.is_none() {
                    return json_answer(400, json!({ "message": "Choose a computer in your setup." }));
                }
                let missing: Vec<Value> = agent["apps"].as_array().into_iter().flatten().filter_map(|id| id.as_str()).filter(|id| !self.connected(id)).map(|id| {
                    let title = self.offered_apps().into_iter().find(|app| app["id"] == json!(id)).map(|app| app["title"].clone()).unwrap_or(json!(id));
                    match &other {
                        Some(name) => json!({ "id": id, "title": title, "connect": false, "words": format!("Connect it on {name} first") }),
                        None => json!({ "id": id, "title": title, "connect": true, "words": "Not connected" }),
                    }
                }).collect();
                let models: Vec<Value> = super::models().into_iter().map(|(mut model, efforts)| {
                    model["side"] = json!("engine");
                    model["efforts"] = json!(efforts);
                    model["aliases"] = json!([]);
                    model
                }).collect();
                let providers: Vec<Value> = super::PROVIDERS.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect();
                let routines: Vec<Value> = agent["routines"].as_array().into_iter().flatten().map(|routine| { let mut routine = routine.clone(); routine["enabled"] = json!(false); routine }).collect();
                let blocked = (!missing.is_empty()).then(|| format!("{} app{} still to connect", missing.len(), if missing.len() == 1 { "" } else { "s" }));
                json_answer(200, json!({ "models": models, "providers": providers, "known": { "engine": true, "agents": true }, "starting": { "providerID": "anthropic", "id": "claude-sonnet-5-5" }, "missing": missing, "routines": routines, "blocked": blocked }))
            }
            ("POST", ["agents", id, "add"]) => {
                let agent = self.agents().into_iter().find(|agent| agent["id"] == json!(id))?;
                if !body["model"].is_object() {
                    return json_answer(400, json!({ "message": "Choose a model and account, and send the original Add request ID." }));
                }
                if agent["apps"].as_array().into_iter().flatten().filter_map(|id| id.as_str()).any(|id| !self.connected(id)) {
                    return json_answer(409, json!({ "message": "Connect the required apps on This Mac before adding this agent." }));
                }
                let made_name = body["name"].as_str().filter(|name| !name.trim().is_empty()).unwrap_or(agent["title"].as_str().unwrap_or_default());
                let made = bots.answer("POST", "kuvo/bots", &|_| None, &json!({ "name": made_name, "model": body["model"], "request": body["request"] }), &[], now);
                let Some((201, made)) = made else { return made.map(|(status, value)| (status, "application/json", value.to_string().into_bytes())) };
                let paused = body["routines"].as_array().is_none_or(Vec::is_empty) && agent["routines"].as_array().is_some_and(|routines| !routines.is_empty());
                let bot_id = made["bot"]["id"].clone();
                if let Some(bot) = bots.bots.iter_mut().find(|bot| bot["id"] == bot_id) {
                    bot["job"] = agent["summary"].clone();
                    bot["paused"] = json!(paused);
                }
                *self.made.entry((*id).to_owned()).or_default() += 1;
                let mut made = made;
                made["bot"]["job"] = agent["summary"].clone();
                made["bot"]["paused"] = json!(paused);
                json_answer(201, made)
            }
            ("POST", ["skills", "preview"]) => {
                let (status, value) = self.preview(body);
                json_answer(status, value)
            }
            ("POST", ["skills", "install"]) => {
                let Some(preview) = body["preview"].as_str().and_then(|id| self.previews.remove(id)) else {
                    return json_answer(400, json!({ "message": "Preview the skill again first." }));
                };
                let skill = &preview["skill"];
                let id = skill["id"].clone();
                self.skills.retain(|each| each["id"] != id);
                let mut kept = json!({ "id": id, "title": skill["name"], "summary": skill["description"], "category": "Your skills", "origin": skill["origin"], "files": preview["files"] });
                if let Some(source) = skill.get("source") {
                    kept["source"] = source.clone();
                }
                self.skills.push(kept);
                json_answer(200, json!({ "id": id, "problems": [] }))
            }
            ("POST", ["skills", "create"]) => {
                let id = body["name"].as_str().unwrap_or_default().trim().to_lowercase().replace(' ', "-");
                let description = body["description"].as_str().unwrap_or_default().trim().to_owned();
                if id.is_empty() || description.is_empty() {
                    return json_answer(400, json!({ "message": "Fill in the name, description and instructions." }));
                }
                let text = skill_md(&id, &description, body["instructions"].as_str().unwrap_or_default());
                self.skills.push(json!({ "id": id, "title": id, "summary": description, "category": "Your skills", "origin": "created", "files": [file("SKILL.md", &text)] }));
                json_answer(200, json!({ "id": id, "problems": [] }))
            }
            ("POST", ["skills", "restore"]) => {
                let Some(at) = self.deleted.iter().position(|each| each["id"] == body["id"]) else {
                    return json_answer(400, json!({ "message": "That isn’t an entry in Recently Deleted." }));
                };
                let entry = self.deleted.remove(at);
                let name = entry["title"].as_str().unwrap_or_default().to_owned();
                let summary = entry["summary"].as_str().unwrap_or_default().to_owned();
                self.skills.push(json!({ "id": name, "title": name, "summary": summary, "category": "Your skills", "origin": "local", "files": [file("SKILL.md", &skill_md(&name, &summary, "Instructions."))] }));
                json_answer(200, json!({}))
            }
            ("POST", ["skills", "places"]) => match body["place"].as_str().map(str::trim).filter(|place| !place.is_empty()) {
                Some(place) => {
                    self.places.push(place.to_owned());
                    json_answer(200, json!({}))
                }
                None => json_answer(400, json!({ "message": "Give a folder or catalog address." })),
            },
            ("DELETE", ["skills", "places"]) => {
                let index = body["index"].as_u64().map(|index| index as usize).filter(|index| *index < self.places.len());
                match index {
                    Some(index) => {
                        self.places.remove(index);
                        json_answer(200, json!({}))
                    }
                    None => json_answer(400, json!({ "message": "Choose a place to remove." })),
                }
            }
            ("POST", ["skills", id, "enabled"]) => {
                let on = body["on"].as_bool()?;
                if on {
                    self.disabled.remove(*id);
                } else {
                    self.disabled.insert((*id).to_owned());
                }
                json_answer(200, json!({ "enabled": on }))
            }
            ("POST", ["skills", id, "add"]) => {
                let skill = self.catalog_skill(id)?;
                if body["version"] != skill["version"] {
                    return json_answer(409, json!({ "message": "Review the current version before installing it." }));
                }
                if !self.skills.iter().any(|each| each["id"] == json!(id)) {
                    self.skills.push(skill);
                }
                json_answer(200, json!({ "installed": true, "problems": [] }))
            }
            ("GET", ["skills", id]) => match self.skill_details(id) {
                Some(details) => json_answer(200, details),
                None => missing(),
            },
            ("PATCH", ["skills", id]) => {
                let Some(skill) = self.skills.iter_mut().find(|each| each["id"] == json!(id)) else { return missing() };
                let description = body["description"].as_str().unwrap_or_default().trim().to_owned();
                if description.is_empty() {
                    return json_answer(400, json!({ "message": "Give the skill a description and instructions." }));
                }
                let text = skill_md(id, &description, body["instructions"].as_str().unwrap_or_default());
                skill["summary"] = json!(description);
                skill["files"] = json!([file("SKILL.md", &text)]);
                json_answer(200, json!({ "installed": true }))
            }
            ("DELETE", ["skills", id]) => {
                let Some(at) = self.skills.iter().position(|each| each["id"] == json!(id)) else { return missing() };
                let skill = self.skills.remove(at);
                if skill["origin"] == json!("local") {
                    self.deleted.push(json!({ "id": format!("{}-{id}", now as u64), "title": id, "summary": skill["summary"] }));
                }
                json_answer(200, json!({ "installed": false }))
            }
            _ => missing(),
        }
    }
}
