//! What the stand-in box says for a first run: whether you've had it (`welcomed` in `kuvo/setup`,
//! set through `POST kuvo/setup/welcomed`), the folders worth starting in
//! (`kuvo/folders/suggested`), a few folders of its home to browse (`kuvo/folders`), and the models
//! that need no account (`free` in `kuvo/models/offered`). All made up.

use serde_json::{Value, json};

use super::HOME;

const DAY: f64 = 24.0 * 60.0 * 60_000.0;

/// How the stand-in's computer stands as a first run meets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// A new computer: no coding agent installed, nothing signed in, only the free models run.
    Nothing,
    /// Several agents: two signed in, one whose sign-in ended, one signed out, one not installed.
    Crowded,
    /// A model runs already and nothing needs you: every sign-in there is works.
    Running,
}

impl Scenario {
    /// As the app's `--first-run <name>` names it.
    pub fn named(name: &str) -> Option<Scenario> {
        match name {
            "nothing" => Some(Scenario::Nothing),
            "crowded" => Some(Scenario::Crowded),
            "running" => Some(Scenario::Running),
            _ => None,
        }
    }
}

/// What the stand-in says for a first run.
#[derive(Clone, Debug)]
pub struct FirstRun {
    /// Whether the first run's been had (as a box says once it has: the stand-in's sample has).
    pub welcomed: bool,
    /// Setting `welcomed` is turned away (`503`).
    pub welcomed_fails: bool,
    /// The models that need no account are offered too.
    pub free: bool,
    /// No account's model runs: only the free ones are offered.
    pub only_free: bool,
    /// It suggests no folder (a new computer).
    pub no_folders: bool,
    /// Its suggested folders can't be read now (`503`).
    pub folders_fail: bool,
}

impl Default for FirstRun {
    fn default() -> FirstRun {
        FirstRun { welcomed: true, welcomed_fails: false, free: false, only_free: false, no_folders: false, folders_fail: false }
    }
}

/// The models that need no account, as the box offers them.
pub fn free_models() -> Vec<(Value, Vec<&'static str>)> {
    vec![
        (json!({ "providerID": "opencode", "id": "big-pickle", "name": "Big Pickle", "free": true }), vec![]),
        (json!({ "providerID": "opencode", "id": "grok-code", "name": "Grok Code Fast", "free": true }), vec![]),
    ]
}

/// The model a new session starts on where only the free ones run.
pub fn free_start() -> Value {
    json!({ "providerID": "opencode", "id": "big-pickle" })
}

/// A folder of the stand-in's home, as `kuvo/folders` lists it: its folders (which are
/// repositories), or why it can't be read. None for a folder that isn't one of these.
fn folder(path: &str) -> Option<Result<Vec<(&'static str, bool)>, &'static str>> {
    let path = path.trim_end_matches('/');
    let inside = if path == "~" || path.is_empty() { "" } else { path.strip_prefix(HOME)? };
    Some(match inside {
        "" => Ok(vec![("Desktop", false), ("Developer", false), ("Documents", false), ("Downloads", false), ("Projects", false)]),
        "/Projects" => Ok(vec![("api", true), ("infra", true), ("mobile", true), ("website", true)]),
        "/Developer" => Ok(vec![("dotfiles", true)]),
        "/Documents" => Ok(vec![("kuvo-notes", false), ("Taxes", false)]),
        "/Downloads" => Ok(Vec::new()),
        // (macOS would ask before a program reads it.)
        "/Desktop" => Err("Operation not permitted"),
        "/Projects/api" | "/Projects/infra" | "/Projects/mobile" | "/Projects/website" | "/Developer/dotfiles" => Ok(vec![("src", false)]),
        "/Documents/kuvo-notes" | "/Documents/Taxes" => Ok(Vec::new()),
        _ => return None,
    })
}

impl FirstRun {
    /// Its answer to `method route`, if it's one of these routes (`now`: the moment its sample is
    /// made at).
    pub fn answer(&mut self, method: &str, route: &str, query: &dyn Fn(&str) -> Option<String>, body: &Value, now: f64) -> Option<(u16, Value)> {
        Some(match (method, route) {
            ("GET", "kuvo/setup") => (200, json!({ "setup": null, "epoch": null, "primary": null, "you": { "key": null, "role": null }, "computers": [], "startsFromOthers": { "value": null, "default": "off" }, "heldBy": [], "welcomed": self.welcomed })),
            ("POST", "kuvo/setup/welcomed") if self.welcomed_fails => (503, json!({ "message": "The box isn’t answering." })),
            ("POST", "kuvo/setup/welcomed") => match body["welcomed"].as_bool() {
                Some(welcomed) => {
                    self.welcomed = welcomed;
                    (200, json!({ "welcomed": welcomed }))
                }
                None => (400, json!({ "message": "Say welcomed as true or false." })),
            },
            ("GET", "kuvo/folders/suggested") if self.folders_fail => (503, json!({ "message": "The box isn’t answering." })),
            ("GET", "kuvo/folders/suggested") => {
                let suggested = |name: &str, inside: &str, repository: bool, seen: Option<f64>| json!({ "path": format!("{HOME}/{inside}/{name}"), "name": name, "repository": repository, "seen": seen.map(|seen| seen as u64) });
                let folders = if self.no_folders {
                    Vec::new()
                } else {
                    vec![
                        suggested("website", "Projects", true, Some(now - DAY)),
                        suggested("api", "Projects", true, Some(now - 3.0 * DAY)),
                        suggested("kuvo-notes", "Documents", false, Some(now - 8.0 * DAY)),
                        suggested("dotfiles", "Developer", true, None),
                        suggested("infra", "Projects", true, None),
                        suggested("mobile", "Projects", true, None),
                    ]
                };
                (200, json!({ "home": HOME, "folders": folders }))
            }
            ("GET", "kuvo/folders") => {
                let path = query("path").unwrap_or_default();
                let full = if path == "~" || path == "~/" { HOME.to_owned() } else { path.trim_end_matches('/').to_owned() };
                match folder(&path)? {
                    Ok(folders) => (200, json!({ "path": full, "home": HOME, "folders": folders.iter().map(|(name, repository)| json!({ "name": name, "repository": repository })).collect::<Vec<_>>(), "files": [] })),
                    Err(reason) => (403, json!({ "message": format!("{full} can’t be read here."), "reason": reason })),
                }
            }
            _ => return None,
        })
    }
}
