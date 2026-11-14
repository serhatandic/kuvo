//! The stand-in box's bots, in a box's own shapes (PROTOCOL.md, `kuvo/bots`): the list with what
//! each is doing now, what it holds for another computer and what one refused; what New Bot offers
//! (`kuvo/bots/templates`); making, changing and deleting one; and a bot's sessions
//! (`kuvo/work?bot=`). As a box does, it decides what a bot may be: the app draws what it says.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::{HOME, HOUR};

const COLORS: &[&str] = &["mint", "sky", "indigo", "violet", "pink", "coral", "orange", "amber", "lime", "graphite"];
const SHAPES: &[&str] = &["round", "squircle", "pebble", "gem", "bloom"];
const FACES: &[&str] = &["calm", "happy", "sleepy", "wink", "curious", "bright"];

/// The stand-in's bots.
#[derive(Default)]
pub struct Bots {
    pub bots: Vec<Value>,
    pub held: Vec<Value>,
    pub refused: Vec<Value>,
    /// Bots that moved to another computer: a change to one is held for it.
    moved: HashMap<String, String>,
    /// New Bot requests answered, by their id.
    made: HashMap<String, Value>,
    next: u64,
    /// Its list can't be read now (503).
    pub down: bool,
    /// Nothing can be made or changed now (503, as with its engine stopped).
    pub stopped: bool,
}

impl Bots {
    /// The sample bots, as they'd be at `now`.
    pub fn sample(now: f64) -> Bots {
        let bot = |id: &str, name: &str, color: &str, shape: &str, job: &str, details: &str, personality: Option<&str>, now_: Value| {
            let short = id.trim_start_matches("bot_");
            let mut bot = json!({
                "id": id, "session": format!("ses_{short}"), "name": name,
                "look": { "color": color, "shape": shape, "face": "calm" },
                "job": job, "jobDetails": details, "paused": false, "watching": 0, "agents": false,
                "model": { "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" },
                "folder": format!("{HOME}/.kuvo-code/bots/{short}"), "created": now - 30.0 * 24.0 * HOUR,
                "now": now_,
            });
            if let Some(personality) = personality {
                bot["personality"] = json!(personality);
            }
            bot
        };
        let mut mira = bot(
            "bot_mira", "Mira", "sky", "round", "Looks after the website",
            "Keeps the sign-up flow working, reviews what changes on the site each morning, and starts a session for anything that breaks.",
            Some("preset.mentor"), json!({ "working": 1, "asking": 1 }),
        );
        mira["project"] = json!(format!("{HOME}/Projects/website"));
        Bots {
            bots: vec![
                mira,
                bot("bot_inbox", "Inbox Helper", "coral", "squircle", "Drafts replies to support mail", "", Some("preset.cheerful"), json!({ "working": 0, "asking": 1 })),
                // One with no session listed (its row shows its job), and one whose newest finished.
                bot("bot_release", "Release Captain", "lime", "pebble", "Ships a beta every Friday", "", None, json!({ "working": 0, "asking": 0 })),
                bot("bot_docs", "Docs Keeper", "violet", "gem", "Keeps the docs matching the code", "", Some("preset.perfectionist"), json!({ "working": 0, "asking": 0, "finished": now - 3.0 * HOUR })),
            ],
            // A rename made from another of your clients for a bot that lives on a Mac that's away.
            held: vec![json!({ "bot": "bot_scout", "box": "key_studio", "computer": "studio-mini", "change": { "name": "Scout", "job": { "line": "Watches CI", "details": "" } }, "since": now - 2.0 * HOUR })],
            refused: Vec::new(),
            moved: HashMap::new(),
            made: HashMap::new(),
            next: 1,
            down: false,
            stopped: false,
        }
    }

    /// How many of its bots have a session waiting on you.
    pub fn asking(&self) -> usize {
        self.bots.iter().filter(|bot| bot["now"]["asking"].as_u64().unwrap_or(0) > 0).count()
    }

    /// `bot` moves to `computer`: it no longer lists here, and a change to it is held for there.
    pub fn move_away(&mut self, bot: &str, computer: &str) {
        self.bots.retain(|each| each["id"] != json!(bot));
        self.moved.insert(bot.to_owned(), computer.to_owned());
    }

    /// Its answer to `method route`, if it's one of the bots' routes. `projects`: the box's, as
    /// New Bot offers them (directory and name).
    pub fn answer(&mut self, method: &str, route: &str, query: &dyn Fn(&str) -> Option<String>, body: &Value, projects: &[(String, String)], now: f64) -> Option<(u16, Value)> {
        let segments: Vec<&str> = route.split('/').collect();
        Some(match (method, segments.as_slice()) {
            ("GET", ["kuvo", "bots"]) if self.down => (503, json!({ "message": "The box isn’t answering." })),
            ("GET", ["kuvo", "bots"]) => {
                let mut said = json!({ "bots": self.bots });
                if !self.held.is_empty() {
                    said["held"] = json!(self.held);
                }
                if !self.refused.is_empty() {
                    said["refused"] = json!(self.refused);
                }
                (200, said)
            }
            ("GET", ["kuvo", "bots", "templates"]) => (200, templates(projects)),
            ("POST", ["kuvo", "bots"]) => self.make(body, projects, now),
            ("PATCH", ["kuvo", "bots", id]) => self.change(id, body, now),
            ("DELETE", ["kuvo", "bots", id]) => self.delete(id, now),
            ("GET", ["kuvo", "work"]) => match query("bot") {
                None => (400, json!({ "message": "Say whose work: bot=<id>." })),
                Some(bot) if !self.bots.iter().any(|each| each["id"] == json!(bot)) => (404, json!({ "message": "There’s no such bot here." })),
                Some(bot) => (200, json!({ "bot": bot, "sessions": work(&bot, now), "answered": [], "today": { "started": 0, "working": 0, "tokens": 0 }, "limits": null })),
            },
            _ => return None,
        })
    }

    fn make(&mut self, body: &Value, projects: &[(String, String)], now: f64) -> (u16, Value) {
        if let Some(made) = body["request"].as_str().and_then(|request| self.made.get(request)) {
            return (201, made.clone());
        }
        if self.stopped {
            return (503, json!({ "message": "The bot couldn’t be made: the engine isn’t running." }));
        }
        let name = body["name"].as_str().unwrap_or_default().trim().to_owned();
        if name.is_empty() {
            return (400, json!({ "message": "A bot needs a name." }));
        }
        let all = templates(projects);
        let template = match body["template"].as_str() {
            None => None,
            Some(id) => match all["templates"].as_array().and_then(|list| list.iter().find(|template| template["id"] == json!(id))) {
                Some(template) => Some(template.clone()),
                None => return (400, json!({ "message": format!("There’s no template “{id}”.") })),
            },
        };
        if let Some(refusal) = unknown(body, projects) {
            return refusal;
        }
        let short = format!("new{}", self.next);
        self.next += 1;
        let look = if body["look"].is_object() { body["look"].clone() } else { template.as_ref().map(|template| template["look"].clone()).unwrap_or(json!({ "color": "mint", "shape": "round", "face": "calm" })) };
        let mut bot = json!({
            "id": format!("bot_{short}"), "session": format!("ses_{short}"), "name": name, "look": look,
            "job": "", "jobDetails": "", "paused": false, "watching": 0, "agents": false,
            "model": if body["model"].is_object() { body["model"].clone() } else { starting() },
            "folder": format!("{HOME}/.kuvo-code/bots/{short}"), "created": now,
        });
        let personality = body["personality"].as_str().map(str::to_owned).or_else(|| template.as_ref().and_then(|template| template["personality"].as_str().map(str::to_owned)));
        if let Some(personality) = personality {
            bot["personality"] = json!(personality);
        }
        if let Some(project) = body["project"].as_str() {
            bot["project"] = json!(project);
        }
        // From a template the bot waits, and its job is for your composer; otherwise it writes first.
        let made = json!({ "bot": bot, "firstMessage": template.map(|template| template["job"].clone()).unwrap_or(Value::Null) });
        // (It lists with what it's doing; what making it answered doesn't say.)
        bot["now"] = json!({ "working": 0, "asking": 0 });
        self.bots.push(bot);
        if let Some(request) = body["request"].as_str() {
            self.made.insert(request.to_owned(), made.clone());
        }
        (201, made)
    }

    fn change(&mut self, id: &str, body: &Value, now: f64) -> (u16, Value) {
        let Some(change) = body.as_object().filter(|change| !change.is_empty()) else {
            return (400, json!({ "message": "Say what to change." }));
        };
        if body.get("name").is_some_and(|name| name.as_str().is_none_or(|name| name.trim().is_empty())) {
            return (400, json!({ "message": "A bot needs a name." }));
        }
        if let Some(refusal) = unknown(body, &[]) {
            return refusal;
        }
        // One that lives on another computer now: held for it, a later change on top.
        if let Some(computer) = self.moved.get(id).cloned() {
            return (202, json!({ "held": self.hold(id, &computer, Some(change), false, now) }));
        }
        if self.stopped {
            return (503, json!({ "message": "The bot couldn’t be changed: the engine isn’t running." }));
        }
        let Some(bot) = self.bots.iter_mut().find(|bot| bot["id"] == json!(id)) else {
            return (404, json!({ "message": "There’s no such bot here." }));
        };
        for (field, value) in change {
            match field.as_str() {
                "job" => {
                    bot["job"] = value["line"].clone();
                    bot["jobDetails"] = value["details"].clone();
                }
                "personality" if value.is_null() => {
                    bot.as_object_mut().map(|bot| bot.remove("personality"));
                }
                _ => bot[field.as_str()] = value.clone(),
            }
        }
        let mut answered = bot.clone();
        answered.as_object_mut().map(|bot| bot.remove("now"));
        (200, json!({ "bot": answered }))
    }

    fn delete(&mut self, id: &str, now: f64) -> (u16, Value) {
        if let Some(computer) = self.moved.get(id).cloned() {
            return (202, json!({ "held": self.hold(id, &computer, None, true, now) }));
        }
        let before = self.bots.len();
        self.bots.retain(|bot| bot["id"] != json!(id));
        if self.bots.len() == before { (404, json!({ "message": "There’s no such bot here." })) } else { (200, json!({})) }
    }

    /// Holds a change (a field at a time, on top of what waits) or a delete (over all of it).
    fn hold(&mut self, id: &str, computer: &str, change: Option<&Map<String, Value>>, delete: bool, now: f64) -> Value {
        let at = self.held.iter().position(|held| held["bot"] == json!(id));
        let mut held = at.map(|at| self.held.remove(at)).unwrap_or(json!({ "bot": id, "box": format!("key_{computer}"), "computer": computer, "since": now }));
        if delete {
            held.as_object_mut().map(|held| held.remove("change"));
            held["delete"] = json!(true);
        } else if let Some(change) = change {
            let mut all = held["change"].as_object().cloned().unwrap_or_default();
            all.extend(change.clone());
            held["change"] = Value::Object(all);
        }
        self.held.push(held.clone());
        held
    }
}

/// What a body names that there isn't: a look, a personality, a model that can't run, a project.
fn unknown(body: &Value, projects: &[(String, String)]) -> Option<(u16, Value)> {
    let look = &body["look"];
    if look.is_object() && (!COLORS.contains(&look["color"].as_str().unwrap_or_default()) || !SHAPES.contains(&look["shape"].as_str().unwrap_or_default()) || !FACES.contains(&look["face"].as_str().unwrap_or_default())) {
        return Some((400, json!({ "message": "There’s no such look." })));
    }
    if let Some(personality) = body["personality"].as_str()
        && !personalities().iter().any(|each| each["id"] == json!(personality))
    {
        return Some((400, json!({ "message": format!("There’s no personality “{personality}”.") })));
    }
    // (As a box turns down a model whose account isn't signed in where the bot lives.)
    if let Some(provider) = body["model"]["providerID"].as_str()
        && !super::models().iter().chain(super::agents_models().iter()).any(|(model, _)| model["providerID"] == json!(provider))
    {
        return Some((422, json!({ "message": "That model can’t run here: its account isn’t signed in on this computer." })));
    }
    if let Some(project) = body["project"].as_str()
        && !projects.is_empty()
        && !projects.iter().any(|(directory, _)| directory == project)
    {
        return Some((400, json!({ "message": "There’s no such project here." })));
    }
    None
}

fn starting() -> Value {
    json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" })
}

fn personalities() -> Vec<Value> {
    [
        ("preset.straight", "Straight Shooter", "Says it plainly, and says what it would do."),
        ("preset.cheerful", "Cheerleader", "Warm and encouraging, glad to help."),
        ("preset.mentor", "Mentor", "Explains its thinking so you learn as it goes."),
        ("preset.perfectionist", "Perfectionist", "Checks twice and sweats the details."),
    ]
    .into_iter()
    .map(|(id, name, description)| json!({ "id": id, "name": name, "description": description, "custom": false }))
    .chain([json!({ "id": "custom.pirate", "name": "Ship’s Cook", "description": "One of your own.", "custom": true })])
    .collect()
}

fn templates(projects: &[(String, String)]) -> Value {
    let template = |id: &str, name: &str, job: &str, color: &str, shape: &str, personality: Option<&str>, wants: bool| {
        let mut template = json!({ "id": id, "name": name, "job": job, "instructions": "", "look": { "color": color, "shape": shape, "face": "calm" }, "wantsProject": wants });
        if let Some(personality) = personality {
            template["personality"] = json!(personality);
        }
        template
    };
    json!({
        "templates": [
            template("builder", "Pixel", "Build what I describe, a step at a time, and show me as you go.", "indigo", "squircle", None, false),
            template("news", "Herald", "Each morning, read the news on what I follow and tell me what matters.", "amber", "round", Some("preset.straight"), false),
            template("inbox", "Courier", "Read my mail twice a day and draft replies to what needs one.", "coral", "pebble", Some("preset.cheerful"), false),
            template("ci", "Keeper", "Watch this project’s checks and fix what breaks.", "mint", "gem", Some("preset.perfectionist"), true),
            template("review", "Lens", "Review each change in this project before it merges.", "violet", "bloom", Some("preset.mentor"), true),
        ],
        "personalities": personalities(),
        "looks": { "colors": COLORS, "shapes": SHAPES, "faces": FACES },
        "startingModel": { "mac": starting() },
        "projects": projects.iter().map(|(directory, name)| json!({ "directory": directory, "name": name })).collect::<Vec<_>>(),
    })
}

/// A bot's sessions, the latest first: the stand-in's own project sessions, working for it.
fn work(bot: &str, now: f64) -> Vec<Value> {
    let session = |id: &str, title: &str, project: &str, state: &str, ago: f64| {
        json!({ "session": id, "title": title, "folder": format!("{HOME}/Projects/{project}"), "project": format!("{HOME}/Projects/{project}"), "startedBy": bot, "depth": 1, "state": state, "started": now - ago, "tokens": 48_210 })
    };
    match bot {
        "bot_mira" => vec![
            session("ses_signup", "Fix the sign-up form wiping on a taken email", "website", "working", 0.5 * HOUR),
            session("ses_cms", "Move the blog to the new CMS", "website", "asking", 26.0 * HOUR),
            session("ses_dark", "Dark mode for the settings page", "website", "finished", 3.0 * HOUR),
            session("ses_lighthouse", "Get the Lighthouse score above 90", "website", "stopped", 73.0 * HOUR),
        ],
        "bot_docs" => vec![session("ses_webhook", "Flaky webhook retry test", "api", "finished", 5.0 * HOUR)],
        _ => Vec::new(),
    }
}

/// What a bot's own conversation holds, for those the samples give one.
pub fn conversation(session: &str, now: f64) -> Option<Vec<Value>> {
    let t = now - 26.0 * HOUR;
    match session {
        "ses_mira" => Some(vec![
            super::user("msg_m1", t, "Morning. Anything on the site I should know about?", None),
            json!({ "type": "assistant", "id": "msg_m2", "time": { "created": t + 1000.0, "completed": t + 12_000.0 }, "agent": "build", "finish": "stop", "content": [
                { "type": "text", "text": "Two things:\n\n- **Sign-up** wipes the form when the email is already taken. I’ve started a session on it.\n- **The blog’s move** to the new CMS is waiting on you: should the old URLs keep working?\n\nEverything else is as you left it." }
            ] }),
            json!({ "type": "idle", "id": "msg_m3", "time": { "created": t + 12_000.0 }, "outcome": "succeeded" }),
        ]),
        _ => None,
    }
}
