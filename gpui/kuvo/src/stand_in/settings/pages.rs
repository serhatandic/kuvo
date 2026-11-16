//! The stand-in's settings as a box describes them (PROTOCOL.md, "A box's settings"): its
//! sections, each one's page made from the same state its older routes answer from, a row changed
//! from the revision it was read at, and what a row offers done. As a box does, it words every
//! row, says what can't be changed here and why, and turns a stale change down with the page.

use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::Settings;

const OWN_CLOCK: &str = "own-clock";
const ZONES: &[&str] = &["America/Los_Angeles", "America/New_York", "America/Sao_Paulo", "Europe/London", "Europe/Berlin", "Europe/Istanbul", "Asia/Dubai", "Asia/Kolkata", "Asia/Tokyo", "Australia/Sydney", "UTC"];
const WHEN: &[(&str, &str, Option<&str>)] = &[
    ("edit", "Edit files", Some("Changing, creating or deleting files. Plan mode doesn’t, by default.")),
    ("shell", "Run commands", Some("In the terminal.")),
    ("read", "Read files", Some("Files ending in .env ask first unless you allow this.")),
    ("external_directory", "Work outside the project", Some("In folders that aren’t the project’s.")),
    ("webfetch", "Open web pages", None),
    ("websearch", "Search the web", None),
    ("subagent", "Hand work to subagents", None),
    ("skill", "Use skills", None),
];
const EFFECTS: &[(&str, &str)] = &[("allow", "Allow"), ("ask", "Ask First"), ("deny", "Don’t Allow")];

fn zone_title(zone: &str) -> String {
    zone.replace('_', " ").replace('/', " › ")
}

fn engine_default(action: &str) -> &'static str {
    if action == "external_directory" { "ask" } else { "allow" }
}

/// A row with its revision: a hash of what it is and whether it can be changed, never its words.
pub(super) fn row(mut row: Value) -> Value {
    let ids = |list: &Value| -> Vec<Value> { list.as_array().map(|list| list.iter().map(|each| each["id"].clone()).collect()).unwrap_or_default() };
    let items: Vec<Value> = row["items"].as_array().map(|items| items.iter().map(|item| item["revision"].clone()).collect()).unwrap_or_default();
    let can = row.get("can").map(|can| can == &json!(true));
    let what = json!([row["id"], row["kind"], row["value"], row["set"], can, ids(&row["choices"]), ids(&row["actions"]), items]).to_string();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    what.hash(&mut hasher);
    row["revision"] = json!(format!("{:016x}", hasher.finish()));
    row
}

fn refused(status: u16, message: &str) -> (u16, Value) {
    (status, json!({ "message": message }))
}

impl Settings {
    /// Its answer to one of `kuvo/settings`' routes; `name` the computer's, `here` whether the
    /// app asking runs beside it.
    pub(super) fn pages_answer(&mut self, method: &str, segments: &[&str], body: &Value, name: &str, here: bool) -> Option<(u16, Value)> {
        let ["kuvo", "settings", rest @ ..] = segments else { return None };
        if let Some(answer) = self.mcp_answer(method, rest, body, name, here) {
            return Some(answer);
        }
        if let Some(answer) = self.agents_answer(method, rest, body) {
            return Some(answer);
        }
        Some(match (method, rest) {
            ("GET", []) if self.pages_down => refused(503, "Kuvo on this computer isn’t answering."),
            ("GET", []) => {
                let mut sections = self.sections();
                if let Some(listed) = sections.as_array_mut() {
                    listed.insert(3.min(listed.len()), Settings::computer_section(name, here));
                }
                (200, json!({ "sections": sections }))
            }
            ("GET", [section]) | ("GET", [section, "events"]) => match self.page(section, name, here) {
                Some(page) => (200, page),
                None => refused(404, "This computer has no settings by that name."),
            },
            ("PATCH", [section]) => {
                let (Some(id), Some(was)) = (body["row"].as_str(), body["was"].as_str()) else { return Some(refused(400, "Say which row, and the revision it was read at.")) };
                let Some(page) = self.page(section, name, here) else { return Some(refused(404, "This computer has no settings by that name.")) };
                let Some(at) = find(&page, id) else { return Some(refused(404, "That isn’t on this page now.")) };
                if let Some(why) = at["can"]["why"].as_str() {
                    return Some(refused(403, why));
                }
                if at["revision"] != json!(was) {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                if let Some(refusal) = self.change(section, &at, &body["value"]) {
                    return Some(refusal);
                }
                (200, self.page(section, name, here)?)
            }
            ("POST", [section, id, action]) => {
                let Some(page) = self.page(section, name, here) else { return Some(refused(404, "This computer has no settings by that name.")) };
                let Some(at) = find(&page, id) else { return Some(refused(404, "That isn’t on this page now.")) };
                let Some(offered) = at["actions"].as_array().and_then(|actions| actions.iter().find(|each| each["id"] == json!(action))) else { return Some(refused(404, "That isn’t offered now.")) };
                if let Some(why) = offered["can"]["why"].as_str() {
                    return Some(refused(403, why));
                }
                if offered.get("opens").is_some() {
                    return Some(refused(400, "The app opens that itself."));
                }
                if !body["was"].is_string() {
                    return Some(refused(400, "Say the revision it was read at."));
                }
                if at["revision"] != body["was"] {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                self.act(id, action);
                (200, self.page(section, name, here)?)
            }
            _ => return None,
        })
    }

    fn sections(&self) -> Value {
        let mut sections = vec![
            json!({ "id": "clock", "title": "Clock", "group": "General", "icon": "clock", "line": "Your time zone, which bots’ wake-ups and times of day go by on all your computers.", "keywords": ["time zone", "timezone", "clock", "wake-ups", "schedule", "travel"] }),
            json!({ "id": "permissions", "title": "Permissions", "group": "Agents & Tools", "icon": "shield", "line": "What agents may do without asking, and the sandbox their commands run in.", "keywords": ["approval", "full access", "auto", "ask first", "allow", "don't allow", "sandbox", "also allowed", "other computers", "rules"] }),
            json!({ "id": "browser", "title": "Browser", "group": "System", "icon": "globe", "line": "The browser’s search engine, and offering to save sign-ins.", "keywords": ["search engine", "google", "duckduckgo", "sign-ins", "passwords", "tabs", "never offered"] }),
        ];
        if self.agent_settings.on {
            sections.push(super::agents::Agents::section());
        }
        if self.mcp.on {
            sections.push(super::mcp::Mcp::section());
        }
        sections.extend(self.more_sections.iter().map(|(section, _)| section.clone()));
        Value::Array(sections)
    }

    /// This computer's own section, titled for who asks: "This Mac" to the app beside it.
    fn computer_section(name: &str, here: bool) -> Value {
        json!({ "id": "computer", "title": if here { "This Mac" } else { name }, "group": "System", "icon": "computer", "line": "Whether Kuvo runs here, its version and updates, and what may connect.", "keywords": ["running", "restart", "version", "update", "devices", "phone", "connect", "login items", "kuvo helper", "screen recording", "accessibility", "macos"] })
    }

    /// This computer's page, as a Mac's own box describes it (PROTOCOL.md, "A box's settings",
    /// This computer): how it stands, its version, your devices let in, and what macOS allows
    /// the program its agents run under.
    fn computer_page(&self, name: &str, here: bool) -> Value {
        let at = &self.computer;
        let place = if here { "this Mac" } else { name };
        let opens_here = |id: &str, title: &str, address: &str| if here { json!({ "id": id, "title": title, "opens": address }) } else { json!({ "id": id, "title": title, "can": { "why": format!("Open it on {name} itself.") } }) };
        let stops = if at.working { format!("Agents are at work on {place} now: what they’re doing stops.") } else { format!("Nothing is at work on {place} now.") };
        let mut rows = vec![row(json!({
            "id": "state", "title": "Running", "kind": "actions",
            "line": if at.at_login { "Kuvo Helper keeps it running, and starts it at login." } else { "Kuvo Helper keeps it running." },
            "actions": [{ "id": "restart", "title": "Restart…", "confirm": { "title": format!("Restart Kuvo on {place}?"), "body": format!("{stops} Kuvo is back in a moment."), "button": "Restart" } }],
        }))];
        let mut update = json!({ "id": "update", "title": if at.ready.is_some() { "Update Now" } else { "Check Now" } });
        if at.working {
            update["confirm"] = json!({ "title": format!("Update Kuvo on {place} now?"), "body": format!("{stops} A newer version, if there is one, installs now."), "button": "Update Now" });
        }
        rows.push(match (&at.updating, &at.ready) {
            (true, _) => row(json!({ "id": "version", "title": format!("Kuvo Code {}", at.version), "line": "A newer version is being installed.", "kind": "status", "busy": "Updating…" })),
            (_, Some(ready)) => row(json!({ "id": "version", "title": format!("Kuvo Code {ready} is ready"), "line": "It installs when nothing’s at work, or now.", "kind": "actions", "value": ready, "actions": [update] })),
            _ => row(json!({ "id": "version", "title": format!("Kuvo Code {}", at.version), "line": "Up to date. It updates itself when nothing’s at work.", "kind": "actions", "value": at.version, "actions": [update] })),
        });
        let mut devices = json!({ "id": "devices", "title": "Let my devices connect", "line": format!("Your phone and other computers reach {place}, end to end encrypted."), "kind": "switch", "value": at.devices, "can": true });
        if !here {
            devices["can"] = json!({ "why": format!("Change it in Kuvo Code on {name} itself.") });
        } else if at.devices {
            devices["confirm"] = json!({ "title": "Stop letting your devices connect?", "body": format!("Your phone, and Kuvo Code on your other computers, can’t reach {place} until you turn this on again."), "button": "Turn Off" });
        }
        rows.push(row(devices));
        let mut groups = vec![json!({ "id": "computer", "rows": rows })];
        let permissions = &at.permissions;
        if permissions.applies {
            let mut allows = vec![if at.at_login {
                row(json!({ "id": "login-items", "title": "Login Items", "line": format!("Kuvo starts when you log in to {place}."), "kind": "status", "value": "On" }))
            } else {
                row(json!({ "id": "login-items", "title": "Login Items", "line": format!("Off, so Kuvo on {place} doesn’t start at login."), "tone": "warning", "kind": "actions", "value": "Off", "actions": [opens_here("open-login-items", "Open Login Items", "x-apple.systempreferences:com.apple.LoginItems-Settings.extension")] }))
            }];
            for (id, title, pane, allowed, is_for, not) in [
                ("screenRecording", "Screen Recording", "Privacy_ScreenCapture", permissions.screen_recording, "For agents that look at the screen.", "Not allowed. Agents can’t look at the screen."),
                ("accessibility", "Accessibility", "Privacy_Accessibility", permissions.accessibility, "For agents that click and type in other apps.", "Not allowed. Agents can’t click or type in other apps."),
            ] {
                let row_id = format!("allows-{id}");
                allows.push(if !permissions.known {
                    row(json!({ "id": row_id, "title": title, "line": format!("Kuvo Helper hasn’t said yet. {is_for}"), "kind": "status", "value": "Not known yet" }))
                } else if allowed {
                    row(json!({ "id": row_id, "title": title, "line": is_for, "kind": "status", "value": "Allowed" }))
                } else {
                    row(json!({ "id": row_id, "title": title, "line": not, "tone": "warning", "kind": "actions", "value": "Not allowed", "actions": [opens_here(&format!("open-{id}"), "Open System Settings", &format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))] }))
                });
            }
            allows.push(row(json!({ "id": "allows-files", "title": "Files and Folders", "line": "macOS asks when an agent first opens Desktop, Documents or Downloads.", "kind": "status", "value": "Asked when needed" })));
            groups.push(json!({ "id": "macos", "title": "What macOS Allows Kuvo Helper", "note": "Kuvo Helper is the program your agents run under. Only you allow these, in System Settings.", "rows": allows }));
        }
        json!({ "id": "computer", "title": if here { "This Mac" } else { name }, "groups": groups })
    }

    fn page(&self, section: &str, name: &str, here: bool) -> Option<Value> {
        match section {
            "clock" => Some(self.clock(name)),
            "permissions" => Some(self.permissions(name, here)),
            "browser" => Some(self.browser_page()),
            "computer" => Some(self.computer_page(name, here)),
            other => self.more_sections.iter().find(|(section, _)| section["id"] == json!(other)).map(|(_, page)| stamped(page.clone())),
        }
    }

    fn clock(&self, name: &str) -> Value {
        let mut choices: Vec<Value> = ZONES.iter().map(|zone| json!({ "id": zone, "title": zone_title(zone) })).collect();
        let line = match &self.zone {
            Some(_) => "Bots’ wake-ups and times of day go by it on all your computers.".to_owned(),
            None => {
                choices.insert(0, json!({ "id": OWN_CLOCK, "title": format!("{name}’s clock ({})", zone_title("Europe/Istanbul")) }));
                format!("None set yet: {name} goes by its own clock. Pick yours, and your other computers take it too.")
            }
        };
        let value = self.zone.clone().unwrap_or_else(|| OWN_CLOCK.to_owned());
        json!({ "id": "clock", "title": "Clock", "groups": [{ "id": "clock", "title": "Clock", "rows": [row(json!({ "id": "time-zone", "title": "Time zone", "line": line, "kind": "choice", "value": value, "choices": choices, "can": true }))] }] })
    }

    fn permissions(&self, name: &str, here: bool) -> Value {
        let approval = &self.approval;
        let full = approval["fullAccess"] == json!(true);
        let mode = match approval["default"].as_str().unwrap_or_default() {
            "askFirst" => "Ask First",
            "acceptEdits" => "Accept Edits",
            "auto" => "Auto",
            "fullAccess" => "Full Access",
            other => other,
        };
        let mut rows = vec![row(json!({ "id": "default-mode", "title": "New sessions start in", "line": "The mode you picked last on this computer. Change it in a conversation’s mode menu.", "kind": "status", "value": mode }))];
        let mut full_access = json!({ "id": "full-access", "title": "Full Access", "kind": "switch", "value": full,
            "line": if full { "Conversations set to Full Access work here without asking." } else { "Asked the first time you pick it in a conversation on this computer." } });
        if full {
            full_access["can"] = json!(true);
            full_access["confirm"] = json!({ "title": "Turn Off Full Access?", "body": format!("Conversations on {name} in Full Access go on in Auto. You’re asked again the next time you pick it."), "button": "Turn Off" });
        } else {
            full_access["can"] = json!({ "why": "Turn it on in a conversation: pick Full Access in its mode menu, and confirm it there." });
        }
        rows.push(row(full_access));
        for (side, words) in [("engine", "the engine"), ("agents", "the coding agents")] {
            let held = &approval["auto"][side]["held"];
            if held.is_object() {
                rows.push(row(json!({ "id": format!("auto-held-{side}"), "title": format!("Auto, for {words}"), "line": held["why"].as_str().or(held["reason"].as_str()).unwrap_or_default(), "tone": "warning", "kind": "status", "value": "Held" })));
            }
        }
        if let Some(other) = approval["otherComputers"].as_bool() {
            rows.push(row(json!({ "id": "other-computers", "title": "Let your other computers start work here", "line": format!("Their bots and agents can start sessions on {name}."), "kind": "switch", "value": other, "can": true })));
        }
        let when: Vec<Value> = WHEN
            .iter()
            .map(|(id, title, line)| {
                let own = self.when.get(*id).cloned();
                let default = engine_default(id);
                let mut choices: Vec<Value> = EFFECTS.iter().map(|(effect, name)| if *effect == default { json!({ "id": effect, "title": format!("{name} (Default)") }) } else { json!({ "id": effect, "title": name }) }).collect();
                if let Some(own) = own.as_deref().filter(|own| !EFFECTS.iter().any(|(effect, _)| effect == own)) {
                    choices.push(json!({ "id": own, "title": "Set by a Newer Engine" }));
                }
                let mut made = json!({ "id": format!("when-{id}"), "title": title, "kind": "choice", "value": own.unwrap_or_else(|| default.to_owned()), "choices": choices, "can": true });
                if let Some(line) = line {
                    made["line"] = json!(line);
                }
                row(made)
            })
            .collect();
        let sandbox = &self.sandbox;
        let needs_you = sandbox["needsYou"] == json!(true);
        let mut sandbox_row = json!({ "id": "sandbox", "title": "Sandbox", "kind": "status", "value": if sandbox["on"] == json!(true) { "On" } else { "Off" },
            "line": if needs_you { sandbox["why"].as_str().unwrap_or_default().to_owned() } else { "Agents’ commands here run in it.".to_owned() } });
        if needs_you {
            sandbox_row["tone"] = json!("warning");
            if let (Some(action), Some(title)) = (sandbox["action"].as_str(), sandbox["actionLabel"].as_str()) {
                let offered = if sandbox["route"].is_string() {
                    let mut offered = json!({ "id": action, "title": title });
                    if action == "restartEngine" && title.ends_with('…') {
                        offered["confirm"] = json!({ "title": format!("Restart {name}’s engine?"), "body": "What agents are doing there now stops. Commands run in the sandbox once it’s back.", "button": "Restart Engine" });
                    }
                    Some(offered)
                } else if sandbox["opensHere"] == json!(true) && !here {
                    Some(json!({ "id": action, "title": title, "can": { "why": format!("Open Software Update on {name} itself.") } }))
                } else {
                    sandbox["opens"].as_str().map(|opens| json!({ "id": action, "title": title, "opens": opens }))
                };
                if let Some(offered) = offered {
                    sandbox_row["kind"] = json!("actions");
                    sandbox_row["actions"] = json!([offered]);
                }
            }
        }
        let items: Vec<Value> = self
            .allowed
            .iter()
            .map(|rule| {
                let whose = rule["owner"]["name"].as_str().map(|owner| format!(" · for {owner}")).unwrap_or_default();
                let line = if rule["gone"] == json!(true) { format!("Gone from this computer{whose}") } else { format!("Can write{whose}") };
                row(json!({ "id": format!("allowed-{}", rule["id"].as_str().unwrap_or_default()), "title": rule["path"], "line": line, "kind": "actions", "actions": [{ "id": "remove", "title": "Remove" }] }))
            })
            .collect();
        json!({ "id": "permissions", "title": "Permissions",
            "groups": [
                { "id": "approval", "title": "Approval", "rows": rows },
                { "id": "when", "title": "When the Agent Wants To", "note": "For every session and mode. Don’t Allow holds even in Full Access. Codex, Cursor, Grok and Antigravity may read files without asking.", "rows": when },
                { "id": "sandbox", "title": "Sandbox", "rows": [row(sandbox_row)] },
                { "id": "allowed", "rows": [row(json!({ "id": "allowed", "title": "Also Allowed", "line": "Folders agents’ commands may write in besides their own. Choosing Always on a stopped command’s card adds one.", "kind": "list", "items": items, "empty": "Nothing else is allowed yet." }))] },
            ],
            "notes": [{ "words": "Rules for particular commands or files, approvals saved with Always, and blocks aren’t on this page yet.", "tone": "quiet" }] })
    }

    fn browser_page(&self) -> Value {
        let settings = &self.browser;
        let never: Vec<Value> = settings["never"]
            .as_array()
            .map(|never| never.iter().filter_map(Value::as_str).map(|host| row(json!({ "id": format!("never-{host}"), "title": host, "kind": "actions", "actions": [{ "id": "offer-again", "title": "Offer Again" }] }))).collect())
            .unwrap_or_default();
        json!({ "id": "browser", "title": "Browser", "groups": [
            { "id": "browser", "rows": [
                row(json!({ "id": "search-engine", "title": "Search engine", "line": "For anything typed in the address field that isn’t an address.", "kind": "choice", "value": settings["searchEngine"], "choices": settings["searchEngines"], "can": true })),
                row(json!({ "id": "tidies", "title": "Close agents’ tabs you haven’t looked at", "line": "An hour after an agent last used one, so tabs don’t pile up.", "kind": "switch", "value": settings["tidiesAgentTabs"] == json!(true), "can": true })),
                row(json!({ "id": "offers", "title": "Offer to save sign-ins", "line": "When you sign in to a site in the browser.", "kind": "switch", "value": settings["offersToSave"] == json!(true), "can": true })),
            ] },
            { "id": "never", "rows": [row(json!({ "id": "never", "title": "Never Offered", "line": "Sites you said Never to when it offered to save a sign-in.", "kind": "list", "empty": "None yet.", "items": never }))] },
        ] })
    }

    /// A row's change, kept as its older route keeps it; a refusal where the value doesn't fit.
    fn change(&mut self, section: &str, at: &Value, value: &Value) -> Option<(u16, Value)> {
        let id = at["id"].as_str().unwrap_or_default();
        let fits_choice = || at["choices"].as_array().is_some_and(|choices| choices.iter().any(|choice| &choice["id"] == value));
        match (section, id) {
            ("clock", "time-zone") => {
                if !fits_choice() {
                    return Some(refused(400, "That isn’t a time zone this computer knows."));
                }
                if value != &json!(OWN_CLOCK) {
                    self.zone = value.as_str().map(str::to_owned);
                }
            }
            ("permissions", "full-access") => {
                if value != &json!(false) {
                    return Some(refused(400, "Full Access is only turned off here."));
                }
                self.approval["fullAccess"] = json!(false);
            }
            ("permissions", "other-computers") => match value.as_bool() {
                Some(allowed) => self.approval["otherComputers"] = json!(allowed),
                None => return Some(refused(400, "A switch is on or off.")),
            },
            ("permissions", when) if when.starts_with("when-") => {
                if !fits_choice() {
                    return Some(refused(400, "That isn’t one of its choices."));
                }
                let action = &when[5..];
                match value.as_str() {
                    Some(effect) if effect != engine_default(action) => self.when.insert(action.to_owned(), effect.to_owned()),
                    _ => self.when.remove(action),
                };
            }
            ("browser", "search-engine") => {
                if !fits_choice() {
                    return Some(refused(400, "That isn’t a search engine the browser knows."));
                }
                self.browser["searchEngine"] = value.clone();
            }
            ("computer", "devices") => match value.as_bool() {
                Some(on) => self.computer.devices = on,
                None => return Some(refused(400, "A switch is on or off.")),
            },
            ("browser", "tidies" | "offers") => {
                if !value.is_boolean() {
                    return Some(refused(400, "A switch is on or off."));
                }
                self.browser[if id == "tidies" { "tidiesAgentTabs" } else { "offersToSave" }] = value.clone();
            }
            // A section a test added: the value is kept as it's given (a secret's, only that one is set).
            _ => {
                for (_, page) in &mut self.more_sections {
                    if page["id"] != json!(section) {
                        continue;
                    }
                    for group in page["groups"].as_array_mut().into_iter().flatten() {
                        for each in group["rows"].as_array_mut().into_iter().flatten() {
                            if each["id"] == json!(id) {
                                if each["kind"] == json!("secret") {
                                    each["set"] = json!(!value.is_null());
                                } else {
                                    each["value"] = value.clone();
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn act(&mut self, id: &str, action: &str) {
        if let (Some(rule), "remove") = (id.strip_prefix("allowed-"), action) {
            self.allowed.retain(|each| each["id"] != json!(rule));
        }
        if let (Some(host), "offer-again") = (id.strip_prefix("never-"), action)
            && let Some(never) = self.browser["never"].as_array_mut()
        {
            never.retain(|each| each != &json!(host));
        }
        match (id, action) {
            ("state", "restart") => self.computer.restarted += 1,
            // (Update Now: the newer version is what it has from then on.)
            ("version", "update") => {
                self.computer.asked_to_update += 1;
                if let Some(ready) = self.computer.ready.take() {
                    self.computer.version = ready;
                }
            }
            _ => {}
        }
        // (The sandbox's own buttons check it again, which changes nothing here.)
    }
}

/// A page a test added, each row given its revision.
fn stamped(mut page: Value) -> Value {
    for group in page["groups"].as_array_mut().into_iter().flatten() {
        for each in group["rows"].as_array_mut().into_iter().flatten() {
            for item in each.get_mut("items").and_then(Value::as_array_mut).into_iter().flatten() {
                *item = row(item.take());
            }
            *each = row(each.take());
        }
    }
    page
}

/// A page's row by id, a list's items too.
fn find(page: &Value, id: &str) -> Option<Value> {
    page["groups"].as_array()?.iter().flat_map(|group| group["rows"].as_array().into_iter().flatten()).flat_map(|row| std::iter::once(row).chain(row["items"].as_array().into_iter().flatten())).find(|row| row["id"] == json!(id)).cloned()
}
