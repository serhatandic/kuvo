//! The stand-in's modes and what runs besides a turn, as a box answers them (PROTOCOL.md,
//! "Approval" and "Work › Background work"): each conversation's mode (this computer's own, Full
//! Access confirmed once here, are `kuvo/approval`'s, which the settings' stand-in keeps and hands
//! in as `here`), each command a conversation left running with what it printed, and each subagent
//! at work under it.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Sender;

use serde_json::{Value, json};

const MODES: [&str; 4] = ["askFirst", "acceptEdits", "auto", "fullAccess"];

#[derive(Default)]
pub struct WorkBars {
    /// Each conversation's mode, as picked.
    pub approvals: HashMap<String, String>,
    /// A coding agent's subagents, by the conversation that started them, whose mode they run with.
    pub parents: HashMap<String, String>,
    /// What runs for each conversation besides its turn: `{commands, subagents}`.
    pub background: BTreeMap<String, Value>,
    /// What each command printed, by its id; one stopped is gone.
    pub printed: HashMap<String, String>,
    /// Commands stopped, in order.
    pub stopped: Vec<String>,
    /// Those following the counts (`kuvo/work/background/events`), one conversation's
    /// (`kuvo/work/background/<session>/events`, with its session) and bots' work (`kuvo/work/events`).
    pub followers: HashMap<u64, Sender<String>>,
    pub session_followers: HashMap<u64, (String, Sender<String>)>,
    pub work_followers: HashMap<u64, Sender<String>>,
    /// When each bot's work last changed.
    pub work_changed: BTreeMap<String, f64>,
}

impl WorkBars {
    /// `session`'s mode as the box shows it, with this computer's (`here`, as `kuvo/approval` says).
    pub fn shown(&self, session: &str, here: &Value) -> Value {
        let approval = self.approvals.get(session).cloned().unwrap_or_else(|| here["default"].as_str().unwrap_or("auto").to_owned());
        let held = here["auto"]["engine"]["held"].clone();
        let full_access = here["fullAccess"] == json!(true);
        let effective = if approval == "auto" && !held.is_null() { "askFirst".to_owned() } else { approval.clone() };
        let options: Vec<Value> = MODES
            .iter()
            .map(|mode| {
                let mut option = json!({ "approval": mode });
                if *mode == "auto" && !held.is_null() {
                    option["held"] = held.clone();
                }
                if *mode == "fullAccess" && !full_access {
                    option["unavailable"] = json!({ "reason": "fullAccessOff", "why": "Confirm it once on this computer first." });
                }
                option
            })
            .collect();
        let mut shown = json!({ "approval": approval, "effective": effective, "stored": true, "plan": false, "options": options });
        if approval == "auto" && !held.is_null() {
            shown["held"] = held;
        }
        shown
    }

    /// How many run for each session, as `kuvo/work/background` counts them.
    pub fn counts(&self) -> Value {
        let mut sessions = serde_json::Map::new();
        for (session, work) in &self.background {
            let (commands, subagents) = (work["commands"].as_array().map_or(0, Vec::len), work["subagents"].as_array().map_or(0, Vec::len));
            if commands + subagents > 0 {
                sessions.insert(session.clone(), json!({ "commands": commands, "subagents": subagents, "tasks": 0, "since": 0 }));
            }
        }
        json!({ "sessions": sessions })
    }

    /// What runs for `session`, each one.
    pub fn of(&self, session: &str) -> Value {
        let mut work = self.background.get(session).cloned().unwrap_or_else(|| json!({ "commands": [], "subagents": [] }));
        if let Some(rows) = work["subagents"].as_array().filter(|rows| !rows.is_empty()) {
            let asking = rows.iter().filter(|row| row["asking"] == true).count();
            let sessions = rows.iter().map(|row| json!({
                "session": row["session"], "title": row["title"].as_str().unwrap_or("Subagent"),
                "state": if row["asking"] == true { "asking" } else { "working" },
            })).collect::<Vec<_>>();
            work["activity"] = json!({ "working": sessions.len() - asking, "asking": asking, "sessions": sessions });
        }
        work
    }

    /// The counts' followers, and each conversation's, hear them as they are now.
    pub fn changed(&mut self) {
        let line = self.counts().to_string();
        self.followers.retain(|_, sender| sender.send(line.clone()).is_ok());
        let lines: Vec<(u64, String)> = self.session_followers.iter().map(|(id, (session, _))| (*id, self.of(session).to_string())).collect();
        for (id, line) in lines {
            if self.session_followers.get(&id).is_some_and(|(_, sender)| sender.send(line).is_err()) {
                self.session_followers.remove(&id);
            }
        }
    }

    /// A bot's work changed: its clients read it again.
    pub fn work_changed(&mut self, bot: &str, at: f64) {
        self.work_changed.insert(bot.to_owned(), at);
        let line = json!({ "changed": self.work_changed }).to_string();
        self.work_followers.retain(|_, sender| sender.send(line.clone()).is_ok());
    }

    /// What it answers of `route`, if it's one of these.
    pub fn answer(&mut self, method: &str, route: &str, body: &Value, folder: Option<&str>, here: &mut Value) -> Option<(u16, Value)> {
        let segments: Vec<&str> = route.split('/').collect();
        Some(match (method, segments.as_slice()) {
            ("GET", ["kuvo", "sessions", session, "approval"]) => match self.parents.get(*session) {
                // (A subagent: its conversation's, which is changed there.)
                Some(parent) => {
                    let mut shown = self.shown(parent, here);
                    shown["parent"] = json!(parent);
                    (200, shown)
                }
                None => (200, self.shown(session, here)),
            },
            ("PUT", ["kuvo", "sessions", session, "approval"]) => {
                let Some(approval) = body.get("approval").and_then(Value::as_str).filter(|mode| MODES.contains(mode)) else {
                    return Some((400, json!({ "message": "That isn't a mode." })));
                };
                if approval == "fullAccess" && here["fullAccess"] != json!(true) {
                    return Some((409, json!({ "message": "Full Access isn’t on for this computer.", "reason": "fullAccessOff" })));
                }
                self.approvals.insert((*session).to_owned(), approval.to_owned());
                // (The last picked is what a new one starts with.)
                here["default"] = json!(approval);
                (200, self.shown(session, here))
            }
            ("GET", ["kuvo", "work", "background"]) => (200, self.counts()),
            ("GET", ["kuvo", "work", "background", session]) => (200, self.of(session)),
            ("POST", ["kuvo", "shell", id, "stop"]) => {
                let id = (*id).to_owned();
                let mut found = false;
                for work in self.background.values_mut() {
                    if let Some(commands) = work["commands"].as_array_mut() {
                        let before = commands.len();
                        commands.retain(|command| command["id"] != json!(id));
                        found |= commands.len() != before;
                    }
                }
                if !found {
                    return Some((404, json!({})));
                }
                self.printed.remove(&id);
                self.stopped.push(id);
                self.changed();
                (200, json!({ "stopped": "signal" }))
            }
            ("GET", ["api", "shell", id, "output"]) | ("GET", ["kuvo", "agents", "api", "shell", id, "output"]) => {
                // (Known only in its own folder, as the engine knows it.)
                let known = self.background.values().flat_map(|work| work["commands"].as_array().cloned().unwrap_or_default()).find(|command| command["id"] == json!(id));
                match (known, self.printed.get(*id)) {
                    (Some(command), Some(printed)) if command["folder"].as_str() == folder || command["agents"] == json!(true) => {
                        (200, json!({ "location": { "directory": folder }, "data": { "output": printed, "cursor": printed.len(), "size": printed.len() } }))
                    }
                    _ => (404, json!({ "message": "Shell command not found" })),
                }
            }
            _ => return None,
        })
    }
}

/// This computer's modes (`kuvo/approval`, `here`) with each one a new conversation can start in,
/// as a box says them (`options`, on the engine and on the coding agents).
pub fn with_options(here: &Value) -> Value {
    let held = here["auto"]["engine"]["held"].clone();
    let full_access = here["fullAccess"] == json!(true);
    let options: Vec<Value> = MODES
        .iter()
        .map(|mode| {
            let mut option = json!({ "approval": mode });
            if *mode == "auto" && !held.is_null() {
                option["held"] = held.clone();
            }
            if *mode == "fullAccess" && !full_access {
                option["unavailable"] = json!({ "reason": "fullAccessOff", "why": "Confirm it once on this computer first." });
            }
            option
        })
        .collect();
    let mut said = here.clone();
    said["options"] = json!({ "engine": options, "agents": options });
    said
}
