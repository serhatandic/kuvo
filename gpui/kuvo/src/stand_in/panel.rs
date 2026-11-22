//! The stand-in's answers for the side panel's tabs, as a box answers them: files found by name
//! (`kuvo/files/find`, ranked as the box ranks them), a command's output as the box cuts it
//! (`kuvo/work/background/<session>/commands/<id>/output`), a conversation's subagents at work and
//! done (`kuvo/work/subagents/<session>`), and its side chats (`kuvo/agents/api/kuvo/btw/…`).

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::git::Git;
use super::work_bars::WorkBars;

#[derive(Default)]
pub struct Panel {
    /// Each conversation's subagents that are done, newest first.
    pub done: BTreeMap<String, Vec<Value>>,
    /// Each conversation's side questions, oldest first.
    pub questions: BTreeMap<String, Vec<Value>>,
    /// Answers the next questions get, in order (none left: it stays being answered).
    pub answers: Vec<String>,
    pub asked: u64,
    /// The stand-in's clock.
    pub now: f64,
}

impl Panel {
    /// `session`'s side threads, as the box groups them: the latest first, each with its questions.
    pub fn threads(&self, session: &str) -> Value {
        let questions = self.questions.get(session).cloned().unwrap_or_default();
        let mut order: Vec<String> = Vec::new();
        let mut threads: HashMap<String, Vec<Value>> = HashMap::new();
        for question in questions {
            let thread = question["thread"].as_str().or(question["id"].as_str()).unwrap_or_default().to_owned();
            if !threads.contains_key(&thread) {
                order.push(thread.clone());
            }
            threads.entry(thread).or_default().push(question);
        }
        let mut listed: Vec<Value> = order
            .into_iter()
            .map(|id| {
                let questions = threads.remove(&id).unwrap_or_default();
                let latest = questions.iter().filter_map(|question| question["answered"].as_f64().or(question["asked"].as_f64())).fold(0.0, f64::max);
                json!({
                    "id": id,
                    "title": questions[0]["question"],
                    "asked": questions[0]["asked"],
                    "latest": latest,
                    "answering": questions.iter().any(|question| question["state"] == "answering"),
                    "questions": questions,
                })
            })
            .collect();
        listed.sort_by(|a, b| b["latest"].as_f64().unwrap_or(0.0).total_cmp(&a["latest"].as_f64().unwrap_or(0.0)));
        json!({ "threads": listed })
    }

    /// What it answers of `route`, if it's one of these.
    pub fn answer(&mut self, method: &str, route: &str, query: impl Fn(&str) -> Option<String>, body: &Value, work: &WorkBars, git: Option<&Git>) -> Option<(u16, Value)> {
        let segments: Vec<&str> = route.split('/').collect();
        Some(match (method, segments.as_slice()) {
            ("GET", ["kuvo", "files", "find"]) => {
                let (Some(directory), Some(text)) = (query("directory"), query("q")) else { return Some((400, json!({ "message": "Give a folder." }))) };
                (200, find(git, &directory, &text))
            }
            ("GET", ["kuvo", "work", "background", session, "commands", id, "output"]) => {
                let command = work.background.get(*session).and_then(|work| work["commands"].as_array().and_then(|commands| commands.iter().find(|command| command["id"] == json!(id)).cloned()));
                match (command, work.printed.get(*id)) {
                    (Some(command), printed) => (200, json!({ "command": command, "output": printed.cloned().unwrap_or_default(), "cut": false, "state": "running" })),
                    (None, _) => (200, json!({ "command": null, "output": "", "cut": false, "state": "ended" })),
                }
            }
            ("GET", ["kuvo", "work", "subagents", session]) => {
                let active: Vec<Value> = work.background.get(*session).and_then(|work| work["subagents"].as_array().cloned()).unwrap_or_default();
                (200, json!({ "active": active, "done": self.done.get(*session).cloned().unwrap_or_default() }))
            }
            ("GET", ["kuvo", "agents", "api", "kuvo", "btw", session, "threads"]) => (200, self.threads(session)),
            ("POST", ["kuvo", "agents", "api", "kuvo", "btw", session]) => {
                let Some(question) = body["question"].as_str().filter(|question| !question.trim().is_empty()) else { return Some((400, json!({ "message": "Ask something." }))) };
                self.asked += 1;
                let id = format!("btw_{}", self.asked);
                let thread = body["thread"].as_str().map(str::to_owned).unwrap_or_else(|| id.clone());
                let asked = self.now + self.asked as f64;
                let mut said = json!({ "id": id, "session": session, "thread": thread, "question": question, "state": "answering", "asked": asked });
                // Its model and its files by name, as the box keeps them.
                if body["model"].is_object() {
                    said["model"] = body["model"].clone();
                }
                if let Some(files) = body["files"].as_array().filter(|files| !files.is_empty()) {
                    let names: Vec<Value> = files.iter().map(|file| file["name"].as_str().map(str::to_owned).unwrap_or_else(|| file["uri"].as_str().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_owned()).into()).collect();
                    said["files"] = json!(names);
                }
                let kept = said.clone();
                if !self.answers.is_empty() {
                    let answer = self.answers.remove(0);
                    said["state"] = json!("answered");
                    said["answer"] = json!(answer);
                    said["answered"] = json!(asked + 1.0);
                }
                self.questions.entry((*session).to_owned()).or_default().push(said);
                (200, json!({ "data": kept }))
            }
            _ => return None,
        })
    }
}

/// The files under `directory` whose path has `text`, as the box ranks them: a name with it before a
/// path with it, then the shorter path; and the page it reads as.
fn find(git: Option<&Git>, directory: &str, text: &str) -> Value {
    let wanted = text.trim().to_lowercase();
    let url = if wanted.starts_with("http://") || wanted.starts_with("https://") {
        Some(text.trim().to_owned())
    } else if wanted.starts_with("localhost") {
        Some(format!("http://{}", text.trim()))
    } else {
        None
    };
    let mut found: Vec<(bool, String, String, String)> = Vec::new();
    if let Some(git) = git
        && !wanted.is_empty()
        && url.is_none()
    {
        let mut folders = vec![directory.trim_end_matches('/').to_owned()];
        while let Some(folder) = folders.pop() {
            let Some(Ok((inside, files))) = git.folders.get(&folder) else { continue };
            folders.extend(inside.iter().map(|name| format!("{folder}/{name}")));
            for name in files {
                let path = format!("{folder}/{name}");
                let relative = path.strip_prefix(&format!("{directory}/")).unwrap_or(&path).to_owned();
                if relative.to_lowercase().contains(&wanted) {
                    let within = relative.rsplit_once('/').map(|(folder, _)| folder.to_owned()).unwrap_or_default();
                    found.push((!name.to_lowercase().contains(&wanted), relative.clone(), path, within));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(&b.1)));
    let files: Vec<Value> = found.into_iter().map(|(_, _, path, folder)| json!({ "path": path, "name": path.rsplit('/').next().unwrap_or(&path), "folder": folder })).collect();
    json!({ "url": url, "files": files })
}
